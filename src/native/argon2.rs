//! Pure-Rust RFC 7693 BLAKE2b and RFC 9106 Argon2id with strict PHC parsing.

use crate::native::tls::crypto::Zeroize;

pub const DEFAULT_ARGON2_M_KIB: u32 = 19_456;
pub const DEFAULT_ARGON2_T: u32 = 2;
pub const DEFAULT_ARGON2_P: u32 = 1;
pub const MAX_ARGON2_M_KIB: u32 = 262_144;
pub const MAX_ARGON2_T: u32 = 64;
pub const MAX_ARGON2_P: u32 = 16;

// ── RFC 7693 Unkeyed BLAKE2b ────────────────────────────────────────────────

const BLAKE2B_IV: [u64; 8] = [
    0x6a09_e667_f3bc_c908,
    0xbb67_ae85_84ca_a73b,
    0x3c6e_f372_fe94_f82b,
    0xa54f_f53a_5f1d_36f1,
    0x510e_527f_ade6_82d1,
    0x9b05_688c_2b3e_6c1f,
    0x1f83_d9ab_fb41_bd6b,
    0x5be0_cd19_137e_2179,
];

const BLAKE2B_SIGMA: [[u8; 16]; 12] = [
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
    [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
    [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
    [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
    [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
    [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
    [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
    [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
    [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
    [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
];

#[inline(always)]
fn blake2b_g(v: &mut [u64; 16], a: usize, b: usize, c: usize, d: usize, x: u64, y: u64) {
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
    v[d] = (v[d] ^ v[a]).rotate_right(32);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(24);
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(63);
}

fn blake2b_compress(h: &mut [u64; 8], block: &[u8; 128], t_lo: u64, t_hi: u64, last: bool) {
    let mut m = [0u64; 16];
    for (i, word) in m.iter_mut().enumerate() {
        let j = i * 8;
        *word = u64::from_le_bytes([
            block[j],
            block[j + 1],
            block[j + 2],
            block[j + 3],
            block[j + 4],
            block[j + 5],
            block[j + 6],
            block[j + 7],
        ]);
    }
    let mut v = [0u64; 16];
    v[..8].copy_from_slice(h);
    v[8..].copy_from_slice(&BLAKE2B_IV);
    v[12] ^= t_lo;
    v[13] ^= t_hi;
    if last {
        v[14] = !v[14];
    }
    for s in &BLAKE2B_SIGMA {
        blake2b_g(&mut v, 0, 4, 8, 12, m[s[0] as usize], m[s[1] as usize]);
        blake2b_g(&mut v, 1, 5, 9, 13, m[s[2] as usize], m[s[3] as usize]);
        blake2b_g(&mut v, 2, 6, 10, 14, m[s[4] as usize], m[s[5] as usize]);
        blake2b_g(&mut v, 3, 7, 11, 15, m[s[6] as usize], m[s[7] as usize]);
        blake2b_g(&mut v, 0, 5, 10, 15, m[s[8] as usize], m[s[9] as usize]);
        blake2b_g(&mut v, 1, 6, 11, 12, m[s[10] as usize], m[s[11] as usize]);
        blake2b_g(&mut v, 2, 7, 8, 13, m[s[12] as usize], m[s[13] as usize]);
        blake2b_g(&mut v, 3, 4, 9, 14, m[s[14] as usize], m[s[15] as usize]);
    }
    for i in 0..8 {
        h[i] ^= v[i] ^ v[i + 8];
    }
    m.zeroize();
    v.zeroize();
}

pub fn blake2b(out: &mut [u8], inputs: &[&[u8]]) {
    assert!((1..=64).contains(&out.len()), "BLAKE2b digest length must be 1..=64");
    let mut h = BLAKE2B_IV;
    h[0] ^= 0x0101_0000 ^ (out.len() as u64);

    let mut buf = [0u8; 128];
    let mut buf_len = 0usize;
    let mut t: u128 = 0;

    for &slice in inputs {
        let mut rem = slice;
        while !rem.is_empty() {
            if buf_len == 128 {
                t = t.wrapping_add(128);
                blake2b_compress(&mut h, &buf, t as u64, (t >> 64) as u64, false);
                buf_len = 0;
            }
            let take = rem.len().min(128 - buf_len);
            buf[buf_len..buf_len + take].copy_from_slice(&rem[..take]);
            buf_len += take;
            rem = &rem[take..];
        }
    }

    t = t.wrapping_add(buf_len as u128);
    buf[buf_len..].fill(0);
    blake2b_compress(&mut h, &buf, t as u64, (t >> 64) as u64, true);

    let mut full = [0u8; 64];
    for (i, &w) in h.iter().enumerate() {
        full[i * 8..i * 8 + 8].copy_from_slice(&w.to_le_bytes());
    }
    out.copy_from_slice(&full[..out.len()]);

    buf.zeroize();
    full.zeroize();
    h.zeroize();
}

fn blake2b_long(out: &mut [u8], first_prefix: &[u8], rest: &[&[u8]]) {
    let t_le = (out.len() as u32).to_le_bytes();
    if out.len() <= 64 {
        let mut h = BLAKE2B_IV;
        h[0] ^= 0x0101_0000 ^ (out.len() as u64);
        let mut buf = [0u8; 128];
        let mut buf_len = 0usize;
        let mut t: u128 = 0;

        for &slice in std::iter::once(&t_le.as_slice())
            .chain(std::iter::once(&first_prefix))
            .chain(rest.iter())
        {
            let mut rem = slice;
            while !rem.is_empty() {
                if buf_len == 128 {
                    t = t.wrapping_add(128);
                    blake2b_compress(&mut h, &buf, t as u64, (t >> 64) as u64, false);
                    buf_len = 0;
                }
                let take = rem.len().min(128 - buf_len);
                buf[buf_len..buf_len + take].copy_from_slice(&rem[..take]);
                buf_len += take;
                rem = &rem[take..];
            }
        }
        t = t.wrapping_add(buf_len as u128);
        buf[buf_len..].fill(0);
        blake2b_compress(&mut h, &buf, t as u64, (t >> 64) as u64, true);
        let mut full = [0u8; 64];
        for (i, &w) in h.iter().enumerate() {
            full[i * 8..i * 8 + 8].copy_from_slice(&w.to_le_bytes());
        }
        out.copy_from_slice(&full[..out.len()]);
        buf.zeroize();
        full.zeroize();
        h.zeroize();
        return;
    }

    let mut v = {
        let mut initial = [0u8; 64];
        // Prepend 4-byte LE output length to inputs via two-slice feed.
        let mut h = BLAKE2B_IV;
        h[0] ^= 0x0101_0000 ^ 64u64;
        let mut buf = [0u8; 128];
        let mut buf_len = 0usize;
        let mut t: u128 = 0;
        for &slice in std::iter::once(&t_le.as_slice())
            .chain(std::iter::once(&first_prefix))
            .chain(rest.iter())
        {
            let mut rem = slice;
            while !rem.is_empty() {
                if buf_len == 128 {
                    t = t.wrapping_add(128);
                    blake2b_compress(&mut h, &buf, t as u64, (t >> 64) as u64, false);
                    buf_len = 0;
                }
                let take = rem.len().min(128 - buf_len);
                buf[buf_len..buf_len + take].copy_from_slice(&rem[..take]);
                buf_len += take;
                rem = &rem[take..];
            }
        }
        t = t.wrapping_add(buf_len as u128);
        buf[buf_len..].fill(0);
        blake2b_compress(&mut h, &buf, t as u64, (t >> 64) as u64, true);
        for (i, &w) in h.iter().enumerate() {
            initial[i * 8..i * 8 + 8].copy_from_slice(&w.to_le_bytes());
        }
        buf.zeroize();
        h.zeroize();
        initial
    };

    out[..32].copy_from_slice(&v[..32]);
    let mut pos = 32usize;
    while out.len() - pos > 64 {
        let mut next = [0u8; 64];
        blake2b(&mut next, &[&v]);
        v = next;
        next.zeroize();
        out[pos..pos + 32].copy_from_slice(&v[..32]);
        pos += 32;
    }
    blake2b(&mut out[pos..], &[&v]);
    v.zeroize();
}

// ── RFC 9106 Argon2id (Version 0x13) ────────────────────────────────────────

type Block = [u64; 128];

#[inline(always)]
fn fbla_mka(x: u64, y: u64) -> u64 {
    let xy = (x & 0xffff_ffff).wrapping_mul(y & 0xffff_ffff);
    x.wrapping_add(y).wrapping_add(xy.wrapping_shl(1))
}

#[inline(always)]
fn gb(a: &mut u64, b: &mut u64, c: &mut u64, d: &mut u64) {
    *a = fbla_mka(*a, *b);
    *d = (*d ^ *a).rotate_right(32);
    *c = fbla_mka(*c, *d);
    *b = (*b ^ *c).rotate_right(24);
    *a = fbla_mka(*a, *b);
    *d = (*d ^ *a).rotate_right(16);
    *c = fbla_mka(*c, *d);
    *b = (*b ^ *c).rotate_right(63);
}

#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn blamka_p(
    v0: &mut u64,
    v1: &mut u64,
    v2: &mut u64,
    v3: &mut u64,
    v4: &mut u64,
    v5: &mut u64,
    v6: &mut u64,
    v7: &mut u64,
    v8: &mut u64,
    v9: &mut u64,
    v10: &mut u64,
    v11: &mut u64,
    v12: &mut u64,
    v13: &mut u64,
    v14: &mut u64,
    v15: &mut u64,
) {
    gb(v0, v4, v8, v12);
    gb(v1, v5, v9, v13);
    gb(v2, v6, v10, v14);
    gb(v3, v7, v11, v15);
    gb(v0, v5, v10, v15);
    gb(v1, v6, v11, v12);
    gb(v2, v7, v8, v13);
    gb(v3, v4, v9, v14);
}

fn permute_argon2_block(r: &mut Block) {
    // Rows (8 rows of 16 u64 words)
    for row in 0..8 {
        let base = row * 16;
        let (a0, rest) = r[base..base + 16].split_at_mut(1);
        let (a1, rest) = rest.split_at_mut(1);
        let (a2, rest) = rest.split_at_mut(1);
        let (a3, rest) = rest.split_at_mut(1);
        let (a4, rest) = rest.split_at_mut(1);
        let (a5, rest) = rest.split_at_mut(1);
        let (a6, rest) = rest.split_at_mut(1);
        let (a7, rest) = rest.split_at_mut(1);
        let (a8, rest) = rest.split_at_mut(1);
        let (a9, rest) = rest.split_at_mut(1);
        let (a10, rest) = rest.split_at_mut(1);
        let (a11, rest) = rest.split_at_mut(1);
        let (a12, rest) = rest.split_at_mut(1);
        let (a13, rest) = rest.split_at_mut(1);
        let (a14, a15) = rest.split_at_mut(1);
        blamka_p(
            &mut a0[0], &mut a1[0], &mut a2[0], &mut a3[0], &mut a4[0], &mut a5[0], &mut a6[0],
            &mut a7[0], &mut a8[0], &mut a9[0], &mut a10[0], &mut a11[0], &mut a12[0],
            &mut a13[0], &mut a14[0], &mut a15[0],
        );
    }
    // Columns (8 columns of 2 adjacent u64 words per row)
    for col in 0..8 {
        let mut t = [0u64; 16];
        for row in 0..8 {
            t[row * 2] = r[row * 16 + col * 2];
            t[row * 2 + 1] = r[row * 16 + col * 2 + 1];
        }
        let [
            mut v0, mut v1, mut v2, mut v3, mut v4, mut v5, mut v6, mut v7,
            mut v8, mut v9, mut v10, mut v11, mut v12, mut v13, mut v14, mut v15,
        ] = t;
        blamka_p(
            &mut v0, &mut v1, &mut v2, &mut v3, &mut v4, &mut v5, &mut v6, &mut v7,
            &mut v8, &mut v9, &mut v10, &mut v11, &mut v12, &mut v13, &mut v14, &mut v15,
        );
        let out = [v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11, v12, v13, v14, v15];
        for row in 0..8 {
            r[row * 16 + col * 2] = out[row * 2];
            r[row * 16 + col * 2 + 1] = out[row * 2 + 1];
        }
    }
}

fn fill_block(prev: &Block, ref_blk: &Block, out: &mut Block) {
    let mut r = [0u64; 128];
    for i in 0..128 {
        r[i] = prev[i] ^ ref_blk[i];
    }
    let z = r;
    permute_argon2_block(&mut r);
    for i in 0..128 {
        out[i] = z[i] ^ r[i];
    }
    r.zeroize();
}

fn fill_block_xor(prev: &Block, ref_blk: &Block, dst: &mut Block) {
    let mut next = [0u64; 128];
    fill_block(prev, ref_blk, &mut next);
    for i in 0..128 {
        dst[i] ^= next[i];
    }
    next.zeroize();
}

fn block_from_bytes(bytes: &[u8; 1024]) -> Block {
    let mut blk = [0u64; 128];
    for (i, w) in blk.iter_mut().enumerate() {
        let j = i * 8;
        *w = u64::from_le_bytes([
            bytes[j],
            bytes[j + 1],
            bytes[j + 2],
            bytes[j + 3],
            bytes[j + 4],
            bytes[j + 5],
            bytes[j + 6],
            bytes[j + 7],
        ]);
    }
    blk
}

fn next_address_block(
    input_block: &mut Block,
    addr_block: &mut Block,
) {
    input_block[6] = input_block[6].wrapping_add(1);
    let zero = [0u64; 128];
    let mut tmp = [0u64; 128];
    fill_block(&zero, input_block, &mut tmp);
    fill_block(&zero, &tmp, addr_block);
    tmp.zeroize();
}

#[allow(clippy::too_many_arguments)]
pub fn argon2id_raw(
    password: &[u8],
    salt: &[u8],
    secret: &[u8],
    ad: &[u8],
    m_cost: u32,
    t_cost: u32,
    parallelism: u32,
    out: &mut [u8],
) -> Result<(), &'static str> {
    if parallelism == 0 || parallelism > MAX_ARGON2_P {
        return Err("invalid Argon2 parallelism");
    }
    if t_cost == 0 || t_cost > MAX_ARGON2_T {
        return Err("invalid Argon2 time cost");
    }
    if m_cost < 8 * parallelism || m_cost > MAX_ARGON2_M_KIB {
        return Err("invalid Argon2 memory cost");
    }
    if salt.len() < 8 || out.len() < 4 || out.len() > 64 {
        return Err("invalid Argon2 salt or tag length");
    }

    let p = parallelism as usize;
    let segment_len = (m_cost / (4 * parallelism)) as usize;
    let lane_len = 4 * segment_len;
    let total_blocks = p * lane_len;

    let mut h0 = [0u8; 64];
    let p_le = parallelism.to_le_bytes();
    let t_len_le = (out.len() as u32).to_le_bytes();
    let m_le = m_cost.to_le_bytes();
    let t_le = t_cost.to_le_bytes();
    let v_le = 19u32.to_le_bytes();
    let y_le = 2u32.to_le_bytes(); // Argon2id
    let pass_len_le = (password.len() as u32).to_le_bytes();
    let salt_len_le = (salt.len() as u32).to_le_bytes();
    let sec_len_le = (secret.len() as u32).to_le_bytes();
    let ad_len_le = (ad.len() as u32).to_le_bytes();

    blake2b(
        &mut h0,
        &[
            &p_le,
            &t_len_le,
            &m_le,
            &t_le,
            &v_le,
            &y_le,
            &pass_len_le,
            password,
            &salt_len_le,
            salt,
            &sec_len_le,
            secret,
            &ad_len_le,
            ad,
        ],
    );

    let mut blocks: Vec<Block> = vec![[0u64; 128]; total_blocks];
    let mut init_bytes = [0u8; 1024];
    for lane in 0..p {
        let lane_le = (lane as u32).to_le_bytes();
        let zero_le = 0u32.to_le_bytes();
        let one_le = 1u32.to_le_bytes();

        blake2b_long(&mut init_bytes, &h0, &[&zero_le, &lane_le]);
        blocks[lane * lane_len] = block_from_bytes(&init_bytes);

        blake2b_long(&mut init_bytes, &h0, &[&one_le, &lane_le]);
        blocks[lane * lane_len + 1] = block_from_bytes(&init_bytes);
    }
    init_bytes.zeroize();

    let mut input_block = [0u64; 128];
    let mut addr_block = [0u64; 128];

    for pass in 0..t_cost {
        for slice in 0..4usize {
            let data_independent = pass == 0 && slice < 2;
            for lane in 0..p {
                if data_independent {
                    input_block.fill(0);
                    input_block[0] = u64::from(pass);
                    input_block[1] = lane as u64;
                    input_block[2] = slice as u64;
                    input_block[3] = total_blocks as u64;
                    input_block[4] = u64::from(t_cost);
                    input_block[5] = 2; // Argon2id
                }

                let start_idx = if pass == 0 && slice == 0 { 2 } else { 0 };
                if data_independent && start_idx % 128 != 0 {
                    next_address_block(&mut input_block, &mut addr_block);
                }
                let lane_base = lane * lane_len;

                for idx in start_idx..segment_len {
                    let cur_offset = slice * segment_len + idx;
                    let prev_offset = if cur_offset == 0 {
                        lane_len - 1
                    } else {
                        cur_offset - 1
                    };

                    let rand_word = if data_independent {
                        if idx % 128 == 0 {
                            next_address_block(&mut input_block, &mut addr_block);
                        }
                        addr_block[idx % 128]
                    } else {
                        blocks[lane_base + prev_offset][0]
                    };

                    let j1 = rand_word as u32;
                    let j2 = (rand_word >> 32) as u32;

                    let ref_lane = if pass == 0 && slice == 0 {
                        lane
                    } else {
                        (j2 as usize) % p
                    };

                    let w_len = if ref_lane == lane {
                        if pass == 0 {
                            cur_offset - 1
                        } else {
                            lane_len - segment_len + idx - 1
                        }
                    } else {
                        let base = if pass == 0 {
                            slice * segment_len
                        } else {
                            lane_len - segment_len
                        };
                        if idx == 0 { base - 1 } else { base }
                    };

                    let start_pos = if pass == 0 || slice == 3 {
                        0
                    } else {
                        (slice + 1) * segment_len
                    };

                    let x = (u64::from(j1).wrapping_mul(u64::from(j1))) >> 32;
                    let y = ((w_len as u64).wrapping_mul(x)) >> 32;
                    let z = (w_len as u64) - 1 - y;
                    let ref_index = (start_pos + z as usize) % lane_len;

                    let prev_block = blocks[lane_base + prev_offset];
                    let ref_block = blocks[ref_lane * lane_len + ref_index];
                    let cur_slot = &mut blocks[lane_base + cur_offset];
                    if pass == 0 {
                        fill_block(&prev_block, &ref_block, cur_slot);
                    } else {
                        fill_block_xor(&prev_block, &ref_block, cur_slot);
                    }
                }
            }
        }
    }

    let mut final_block = blocks[lane_len - 1];
    for lane in 1..p {
        let last = &blocks[lane * lane_len + lane_len - 1];
        for i in 0..128 {
            final_block[i] ^= last[i];
        }
    }

    let mut final_bytes = [0u8; 1024];
    for (i, &w) in final_block.iter().enumerate() {
        final_bytes[i * 8..i * 8 + 8].copy_from_slice(&w.to_le_bytes());
    }
    blake2b_long(out, &final_bytes, &[]);

    h0.zeroize();
    input_block.zeroize();
    addr_block.zeroize();
    final_block.zeroize();
    final_bytes.zeroize();
    for blk in blocks.iter_mut() {
        blk.zeroize();
    }

    Ok(())
}

// ── Strict Unpadded Base64 & PHC String Format ──────────────────────────────

const B64_STD_ALPHA: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

const B64_STD_LUT: [i8; 256] = {
    let mut lut = [-1i8; 256];
    let mut i = 0usize;
    while i < 64 {
        lut[B64_STD_ALPHA[i] as usize] = i as i8;
        i += 1;
    }
    lut
};

pub(crate) fn encode_b64_unpadded(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity((bytes.len() * 4).div_ceil(3));
    let (chunks, rem) = bytes.as_chunks::<3>();
    for &[b0, b1, b2] in chunks {
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.extend_from_slice(&[
            B64_STD_ALPHA[((n >> 18) & 63) as usize],
            B64_STD_ALPHA[((n >> 12) & 63) as usize],
            B64_STD_ALPHA[((n >> 6) & 63) as usize],
            B64_STD_ALPHA[(n & 63) as usize],
        ]);
    }
    match rem.len() {
        1 => {
            let n = (rem[0] as u32) << 16;
            out.extend_from_slice(&[
                B64_STD_ALPHA[((n >> 18) & 63) as usize],
                B64_STD_ALPHA[((n >> 12) & 63) as usize],
            ]);
        }
        2 => {
            let n = ((rem[0] as u32) << 16) | ((rem[1] as u32) << 8);
            out.extend_from_slice(&[
                B64_STD_ALPHA[((n >> 18) & 63) as usize],
                B64_STD_ALPHA[((n >> 12) & 63) as usize],
                B64_STD_ALPHA[((n >> 6) & 63) as usize],
            ]);
        }
        _ => {}
    }
    // SAFETY: `B64_STD_ALPHA` contains ASCII only.
    unsafe { String::from_utf8_unchecked(out) }
}

pub(crate) fn decode_b64_unpadded(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    if bytes.is_empty() || bytes.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity((bytes.len() * 3) / 4);
    let (chunks, rem) = bytes.as_chunks::<4>();
    for &[c0, c1, c2, c3] in chunks {
        let v0 = B64_STD_LUT[c0 as usize];
        let v1 = B64_STD_LUT[c1 as usize];
        let v2 = B64_STD_LUT[c2 as usize];
        let v3 = B64_STD_LUT[c3 as usize];
        if (v0 | v1 | v2 | v3) < 0 {
            return None;
        }
        let n = ((v0 as u32) << 18) | ((v1 as u32) << 12) | ((v2 as u32) << 6) | (v3 as u32);
        out.push((n >> 16) as u8);
        out.push((n >> 8) as u8);
        out.push(n as u8);
    }
    match rem.len() {
        2 => {
            let v0 = B64_STD_LUT[rem[0] as usize];
            let v1 = B64_STD_LUT[rem[1] as usize];
            if (v0 | v1) < 0 || (v1 & 0x0f) != 0 {
                return None;
            }
            out.push(((v0 as u8) << 2) | ((v1 as u8) >> 4));
        }
        3 => {
            let v0 = B64_STD_LUT[rem[0] as usize];
            let v1 = B64_STD_LUT[rem[1] as usize];
            let v2 = B64_STD_LUT[rem[2] as usize];
            if (v0 | v1 | v2) < 0 || (v2 & 0x03) != 0 {
                return None;
            }
            out.push(((v0 as u8) << 2) | ((v1 as u8) >> 4));
            out.push(((v1 as u8) << 4) | ((v2 as u8) >> 2));
        }
        _ => {}
    }
    Some(out)
}

pub fn format_argon2id_phc(
    m_cost: u32,
    t_cost: u32,
    parallelism: u32,
    salt: &[u8],
    tag: &[u8],
) -> String {
    format!(
        "$argon2id$v=19$m={},t={},p={}${}${}",
        m_cost,
        t_cost,
        parallelism,
        encode_b64_unpadded(salt),
        encode_b64_unpadded(tag)
    )
}

pub struct ParsedArgon2id {
    pub m_cost: u32,
    pub t_cost: u32,
    pub parallelism: u32,
    pub salt: Vec<u8>,
    pub hash: Vec<u8>,
}

pub fn parse_argon2id_phc(stored: &str) -> Option<ParsedArgon2id> {
    let mut parts = stored.split('$');
    let (Some(""), Some("argon2id"), Some("v=19"), Some(params), Some(salt_b64), Some(hash_b64), None) =
        (parts.next(), parts.next(), parts.next(), parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };

    let mut p_parts = params.split(',');
    let (Some(m_kv), Some(t_kv), Some(p_kv), None) =
        (p_parts.next(), p_parts.next(), p_parts.next(), p_parts.next())
    else {
        return None;
    };
    let m_cost = m_kv.strip_prefix("m=")?.parse::<u32>().ok()?;
    let t_cost = t_kv.strip_prefix("t=")?.parse::<u32>().ok()?;
    let parallelism = p_kv.strip_prefix("p=")?.parse::<u32>().ok()?;

    if !(1..=MAX_ARGON2_P).contains(&parallelism)
        || !(1..=MAX_ARGON2_T).contains(&t_cost)
        || m_cost < 8 * parallelism
        || m_cost > MAX_ARGON2_M_KIB
    {
        return None;
    }

    let salt = decode_b64_unpadded(salt_b64)?;
    let hash = decode_b64_unpadded(hash_b64)?;
    if !(8..=64).contains(&salt.len()) || !(16..=64).contains(&hash.len()) {
        return None;
    }

    Some(ParsedArgon2id {
        m_cost,
        t_cost,
        parallelism,
        salt,
        hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::crypto::encode_hex;

    #[test]
    fn blake2b_rfc7693_vectors() {
        let mut out = [0u8; 64];
        blake2b(&mut out, &[b"abc"]);
        assert_eq!(
            encode_hex(&out),
            "ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d17d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923"
        );
        blake2b(&mut out, &[b""]);
        assert_eq!(
            encode_hex(&out),
            "786a02f742015903c6c6fd852552d272912f4740e15847618a86e217f71f5419d25e1031afee585313896444934eb04b903a685b1448b755d56f701afe9be2ce"
        );
        // Split input across chunks > 128 bytes
        let big = [0x42u8; 257];
        let mut full_out = [0u8; 32];
        let mut split_out = [0u8; 32];
        blake2b(&mut full_out, &[&big]);
        blake2b(&mut split_out, &[&big[..100], &big[100..256], &big[256..]]);
        assert_eq!(full_out, split_out);
    }

    #[test]
    fn argon2id_matches_rfc9106_and_openssl_reference_vectors() {
        // RFC 9106 §5.3 official Argon2id test vector (secret + ad + 4 lanes)
        let mut out = [0u8; 32];
        argon2id_raw(&[1u8; 32], &[2u8; 16], &[3u8; 8], &[4u8; 12], 32, 3, 4, &mut out).unwrap();
        assert_eq!(
            encode_hex(&out),
            "0d640df58d78766c08c037a34a8b53c9d01ef0452d75b65eb52520e96b01e659"
        );

        // OWASP 19 MiB default vector (p=1, m=19456, t=2) verified against OpenSSL 4.0.3
        let salt_owasp: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        argon2id_raw(
            b"correct horse battery staple",
            &salt_owasp,
            &[],
            &[],
            19_456,
            2,
            1,
            &mut out,
        )
        .unwrap();
        assert_eq!(
            encode_hex(&out),
            "818259b6310026a8e0dbac5d2e6927abcfdb07b32258fac4f61b18b80f929085"
        );

        // Multi-lane & 4-pass vector verified against OpenSSL 4.0.3
        argon2id_raw(b"multi-lane-test", b"0123456789abcdef", &[], &[], 128, 4, 2, &mut out).unwrap();
        assert_eq!(
            encode_hex(&out),
            "2ac040270d413bd6cd9747dfc9600f734848817b2da1a26ba505bc3815405d61"
        );

        // Single-lane vector verified against OpenSSL 4.0.3
        argon2id_raw(b"password", b"somesalt", &[], &[], 64, 2, 1, &mut out).unwrap();
        assert_eq!(
            encode_hex(&out),
            "16a1a498734609dd01456da406de9f3d9da93e6c86c300a12fc1465214ce4922"
        );
    }

    #[test]
    fn phc_roundtrip_and_strict_rejections() {
        let salt = [7u8; 16];
        let tag = [9u8; 32];
        let phc = format_argon2id_phc(DEFAULT_ARGON2_M_KIB, DEFAULT_ARGON2_T, DEFAULT_ARGON2_P, &salt, &tag);
        assert!(phc.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        let mut phc_tail = phc.rsplit('$');
        let tag_part = phc_tail.next().unwrap();
        let salt_part = phc_tail.next().unwrap();
        assert!(!salt_part.contains('=') && !tag_part.contains('='));
        let parsed = parse_argon2id_phc(&phc).unwrap();
        assert_eq!(parsed.m_cost, DEFAULT_ARGON2_M_KIB);
        assert_eq!(parsed.t_cost, DEFAULT_ARGON2_T);
        assert_eq!(parsed.parallelism, DEFAULT_ARGON2_P);
        assert_eq!(parsed.salt, salt);
        assert_eq!(parsed.hash, tag);

        // Reject wrong algorithm/version, padded base64, short salt, DoS memory cost
        assert!(parse_argon2id_phc("$argon2i$v=19$m=19456,t=2,p=1$BwcHBwcHBwcHBwcHBwcHBw$CQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQk").is_none());
        assert!(parse_argon2id_phc("$argon2id$v=18$m=19456,t=2,p=1$BwcHBwcHBwcHBwcHBwcHBw$CQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQk").is_none());
        assert!(parse_argon2id_phc("$argon2id$v=19$m=19456,t=2,p=1$BwcHBwcHBwcHBwcHBwcHBw==$CQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQk").is_none());
        assert!(parse_argon2id_phc("$argon2id$v=19$m=99999999,t=2,p=1$BwcHBwcHBwcHBwcHBwcHBw$CQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQk").is_none());
        assert!(parse_argon2id_phc("$argon2id$v=19$m=19456,t=2,p=1$AAAA$CQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQk").is_none());
    }
}
