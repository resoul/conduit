use std::sync::Arc;

use crate::ir::Module;
use crate::native::validation::{box_scalar_val, is_err_or_nil_obj};
use crate::vm::exec_cold::err_msg;
use crate::vm::opcode::Op;
use crate::vm::value::{
    ListStorage, Obj, SCHEMA_BOXED_BOOL, SCHEMA_BOXED_FLOAT, SCHEMA_BOXED_INT, SCHEMA_CRYPTO_ERROR,
    SCHEMA_INDEX_ERROR, SCHEMA_JSON_DECODE_ERROR, SCHEMA_JSON_ENCODE_ERROR, SCHEMA_KEY_ERROR,
    SCHEMA_PARSE_ERROR, SCHEMA_TIME_ERROR, SCHEMA_VALIDATION_ERROR, SCHEMA_VALIDATION_ISSUE,
    SCHEMA_VALIDATION_VIOLATION, StructData, append_float_cow, format_float,
};
use crate::vm::{CallFrame, RuntimeError, Vm};

#[inline]
pub(crate) fn unbox_scalar_val(obj: &Obj) -> Result<u64, RuntimeError> {
    match obj {
        Obj::Record(rec)
            if matches!(
                rec.schema_id,
                SCHEMA_BOXED_INT | SCHEMA_BOXED_FLOAT | SCHEMA_BOXED_BOOL
            ) =>
        {
            Ok(rec.inline_scalars[0])
        }
        _ => Err(err_msg("Expected boxed scalar value")),
    }
}

#[inline]
pub(crate) fn is_try_fail_obj(module: &Module, obj: &Obj, empty_is_err: bool) -> bool {
    if !empty_is_err && matches!(obj, Obj::Empty) {
        return false;
    }
    is_err_or_nil_obj(module, obj)
}

#[cold]
#[inline(never)]
pub(crate) fn format_try_error(obj: &Obj) -> RuntimeError {
    match obj {
        Obj::Empty => err_msg("Unexpected nil value unwrapped with '?'"),
        Obj::Record(rec) => match rec.schema_id {
            SCHEMA_TIME_ERROR | SCHEMA_JSON_ENCODE_ERROR | SCHEMA_INDEX_ERROR => {
                match rec.get_obj(0) {
                    Obj::Str(msg) => err_msg(msg),
                    _ => err_msg("Runtime error unwrapped with '?'"),
                }
            }
            SCHEMA_KEY_ERROR | SCHEMA_PARSE_ERROR => match rec.get_obj(1) {
                Obj::Str(msg) => err_msg(msg),
                _ => err_msg("Runtime error unwrapped with '?'"),
            },
            SCHEMA_JSON_DECODE_ERROR => {
                let path = match rec.get_obj(0) {
                    Obj::Str(s) => s.as_str(),
                    _ => "",
                };
                let msg = match rec.get_obj(1) {
                    Obj::Str(s) => s.as_str(),
                    _ => "decode error",
                };
                if path.is_empty() {
                    err_msg(msg)
                } else {
                    err_msg(&format!("{} at {}", msg, path))
                }
            }
            SCHEMA_VALIDATION_ISSUE => {
                let code = match rec.get_obj(0) {
                    Obj::Str(s) => s.as_str(),
                    _ => "issue",
                };
                let msg = match rec.get_obj(1) {
                    Obj::Str(s) => s.as_str(),
                    _ => "validation failed",
                };
                err_msg(&format!("Validation issue [{}]: {}", code, msg))
            }
            SCHEMA_VALIDATION_ERROR => {
                if let Obj::List(ListStorage::Obj(items)) = rec.get_obj(0)
                    && let Some(Obj::Record(v)) = items.first()
                    && v.schema_id == SCHEMA_VALIDATION_VIOLATION
                {
                    let path = match v.get_obj(0) {
                        Obj::Str(s) => s.as_str(),
                        _ => "",
                    };
                    let code = match v.get_obj(1) {
                        Obj::Str(s) => s.as_str(),
                        _ => "",
                    };
                    let msg = match v.get_obj(2) {
                        Obj::Str(s) => s.as_str(),
                        _ => "validation failed",
                    };
                    return err_msg(&format!(
                        "Validation failed ({} error(s), first at '{}': [{}] {})",
                        items.len(),
                        path,
                        code,
                        msg
                    ));
                }
                err_msg("ValidationError unwrapped with '?'")
            }
            SCHEMA_CRYPTO_ERROR => {
                let kind = match rec.get_obj(0) {
                    Obj::Str(s) => s.as_str(),
                    _ => "",
                };
                let msg = match rec.get_obj(1) {
                    Obj::Str(s) => s.as_str(),
                    _ => "Crypto error unwrapped with '?'",
                };
                if kind == "overloaded" {
                    RuntimeError::overloaded(msg, 1)
                } else {
                    err_msg(msg)
                }
            }
            _ => {
                if !rec.inline_objs.is_empty()
                    && let Obj::Str(s) = rec.get_obj(0)
                {
                    return err_msg(s);
                }
                err_msg("Error value unwrapped with '?'")
            }
        },
        _ => err_msg("Unexpected failure value unwrapped with '?'"),
    }
}

impl Vm {
    #[inline(always)]
    pub(crate) fn finish_return_frame(&mut self, o_base: usize, num_objs: u16, base_depth: usize) -> Option<CallFrame> {
        if num_objs > 0 {
            self.clear_obj_range(o_base, num_objs as usize);
        }
        self.pop_frame(base_depth)
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn exec_for_list_float_next(
        &mut self,
        s_base: usize,
        o_base: usize,
        list_obj: u16,
        idx_s: u16,
        elem_s: u16,
    ) -> Result<bool, RuntimeError> {
        let idx = self.scalars[s_base + idx_s as usize] as usize;
        match &self.objs[o_base + list_obj as usize] {
            Obj::List(ListStorage::Float(arc)) => {
                if idx < arc.len() {
                    self.scalars[s_base + elem_s as usize] = arc[idx].to_bits();
                    self.scalars[s_base + idx_s as usize] = (idx + 1) as u64;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            _ => Err(err_msg("Expected [Int] or [Float] list in ForListInt")),
        }
    }

    #[inline(never)]
    pub(crate) fn exec_ext_op(
        &mut self,
        s_base: usize,
        o_base: usize,
        op: Op,
    ) -> Result<(), RuntimeError> {
        match op {
            Op::NewListFloat { dst_obj, cap } => {
                self.objs[o_base + dst_obj as usize] = Obj::List(ListStorage::new_float(cap as usize));
            }
            Op::ListAppendFloat { list_obj, val_s } => {
                let val = f64::from_bits(self.scalars[s_base + val_s as usize]);
                match &mut self.objs[o_base + list_obj as usize] {
                    Obj::List(ListStorage::Float(arc)) => {
                        if let Some(vec) = Arc::get_mut(arc) {
                            vec.push(val);
                        } else {
                            append_float_cow(arc, val);
                        }
                    }
                    _ => return Err(err_msg("Expected [Float] list in ListAppendFloat")),
                }
            }
            Op::ScalarToStr { dst_obj, src_s, tag } => {
                let raw = self.scalars[s_base + src_s as usize];
                let s = match tag {
                    1 => (raw as i64).to_string(),
                    2 => format_float(f64::from_bits(raw)),
                    _ => (raw != 0).to_string(),
                };
                self.objs[o_base + dst_obj as usize] = Obj::str(s);
            }
            Op::ObjToStr { dst_obj, src_obj } => {
                let s = self.objs[o_base + src_obj as usize].format_obj();
                self.objs[o_base + dst_obj as usize] = Obj::str(s);
            }
            Op::StrMethod { dst_obj, src_obj, kind } => {
                let res = match &self.objs[o_base + src_obj as usize] {
                    Obj::Str(s) => match kind {
                        0 => s.to_uppercase(),
                        1 => s.to_lowercase(),
                        _ => s.trim().to_string(),
                    },
                    _ => return Err(err_msg("Expected String in StrMethod")),
                };
                self.objs[o_base + dst_obj as usize] = Obj::str(res);
            }
            Op::StrReplace { dst_obj, str_obj, from_obj, to_obj } => {
                let res = match (
                    &self.objs[o_base + str_obj as usize],
                    &self.objs[o_base + from_obj as usize],
                    &self.objs[o_base + to_obj as usize],
                ) {
                    (Obj::Str(s), Obj::Str(from), Obj::Str(to)) => {
                        if from.is_empty() {
                            s.to_string()
                        } else {
                            s.replace(from.as_str(), to.as_str())
                        }
                    }
                    _ => return Err(err_msg("Expected String in StrReplace")),
                };
                self.objs[o_base + dst_obj as usize] = Obj::str(res);
            }
            Op::ClampScalar { dst_s, val_s, lo_s, hi_s, is_float } => {
                let out = if is_float {
                    let val = f64::from_bits(self.scalars[s_base + val_s as usize]);
                    let lo = f64::from_bits(self.scalars[s_base + lo_s as usize]);
                    let hi = f64::from_bits(self.scalars[s_base + hi_s as usize]);
                    if lo.is_nan() || hi.is_nan() || lo > hi {
                        return Err(err_msg("clamp() called with invalid bounds (lo > hi or NaN)"));
                    }
                    val.clamp(lo, hi).to_bits()
                } else {
                    let val = self.scalars[s_base + val_s as usize] as i64;
                    let lo = self.scalars[s_base + lo_s as usize] as i64;
                    let hi = self.scalars[s_base + hi_s as usize] as i64;
                    if lo > hi {
                        return Err(err_msg("clamp() called with lo > hi"));
                    }
                    val.clamp(lo, hi) as u64
                };
                self.scalars[s_base + dst_s as usize] = out;
            }
            Op::ListIndex { dst_obj, list_obj, idx_s, elem_tag } => {
                let idx = self.scalars[s_base + idx_s as usize] as i64;
                let res = list_get_or_index(&self.objs[o_base + list_obj as usize], idx, elem_tag, true)?;
                self.objs[o_base + dst_obj as usize] = res;
            }
            Op::ListGet { dst_obj, list_obj, idx_s, elem_tag } => {
                let idx = self.scalars[s_base + idx_s as usize] as i64;
                let res = list_get_or_index(&self.objs[o_base + list_obj as usize], idx, elem_tag, false)?;
                self.objs[o_base + dst_obj as usize] = res;
            }
            Op::ListContainsScalar { dst_s, list_obj, val_s, elem_tag } => {
                let raw = self.scalars[s_base + val_s as usize];
                let found = match &self.objs[o_base + list_obj as usize] {
                    Obj::List(ListStorage::Int(v)) => {
                        if elem_tag == 3 { v.contains(&((raw != 0) as i64)) } else { v.contains(&(raw as i64)) }
                    }
                    Obj::List(ListStorage::Float(v)) => {
                        let target = f64::from_bits(raw);
                        v.contains(&target)
                    }
                    Obj::List(ListStorage::Obj(v)) => {
                        let kind = match elem_tag { 2 => 1, 3 => 2, _ => 0 };
                        let needle = box_scalar_val(raw, kind);
                        v.contains(&needle)
                    }
                    _ => return Err(err_msg("Expected List in ListContainsScalar")),
                };
                self.scalars[s_base + dst_s as usize] = u64::from(found);
            }
            Op::ListContainsObj { dst_s, list_obj, val_obj } => {
                let needle = &self.objs[o_base + val_obj as usize];
                let found = match &self.objs[o_base + list_obj as usize] {
                    Obj::List(ListStorage::Obj(v)) => v.contains(needle),
                    _ => return Err(err_msg("Expected [Obj] list in ListContainsObj")),
                };
                self.scalars[s_base + dst_s as usize] = u64::from(found);
            }
            Op::ListSlice { dst_obj, list_obj, start_s, end_s } => {
                let start_i = self.scalars[s_base + start_s as usize] as i64;
                let end_i = self.scalars[s_base + end_s as usize] as i64;
                let out = list_slice_val(&self.objs[o_base + list_obj as usize], start_i, end_i)?;
                self.objs[o_base + dst_obj as usize] = out;
            }
            Op::ListReverse { dst_obj, list_obj } => {
                let out = list_reverse_val(&self.objs[o_base + list_obj as usize])?;
                self.objs[o_base + dst_obj as usize] = out;
            }
            Op::ListEnumerate { dst_obj, list_obj, elem_tag } => {
                let out = list_enumerate_val(&self.objs[o_base + list_obj as usize], elem_tag)?;
                self.objs[o_base + dst_obj as usize] = out;
            }
            Op::ListJoin { dst_obj, list_obj, sep_obj } => {
                let sep = match &self.objs[o_base + sep_obj as usize] {
                    Obj::Str(s) => s.as_str(),
                    _ => return Err(err_msg("Expected String separator in ListJoin")),
                };
                let joined = match &self.objs[o_base + list_obj as usize] {
                    Obj::List(ListStorage::Obj(v)) => {
                        let mut parts = Vec::with_capacity(v.len());
                        for item in v.iter() {
                            match item {
                                Obj::Str(s) => parts.push(s.as_str()),
                                _ => return Err(err_msg("List.join requires [String] elements")),
                            }
                        }
                        Obj::str(parts.join(sep))
                    }
                    _ => return Err(err_msg("Expected [String] list in ListJoin")),
                };
                self.objs[o_base + dst_obj as usize] = joined;
            }
            Op::TupleGetScalar { dst_s, tuple_obj, idx } => {
                let elem = match &self.objs[o_base + tuple_obj as usize] {
                    Obj::List(ListStorage::Obj(v)) => v
                        .get(idx as usize)
                        .ok_or_else(|| err_msg("Tuple index out of bounds"))?,
                    _ => return Err(err_msg("Expected Tuple in TupleGetScalar")),
                };
                self.scalars[s_base + dst_s as usize] = unbox_scalar_val(elem)?;
            }
            Op::TupleGetObj { dst_obj, tuple_obj, idx } => {
                let elem = match &self.objs[o_base + tuple_obj as usize] {
                    Obj::List(ListStorage::Obj(v)) => v
                        .get(idx as usize)
                        .cloned()
                        .ok_or_else(|| err_msg("Tuple index out of bounds"))?,
                    _ => return Err(err_msg("Expected Tuple in TupleGetObj")),
                };
                self.objs[o_base + dst_obj as usize] = elem;
            }
            _ => unreachable!("unexpected op in exec_ext_op"),
        }
        Ok(())
    }
}

pub(crate) fn list_get_or_index(list: &Obj, idx: i64, elem_tag: u8, is_index_err: bool) -> Result<Obj, RuntimeError> {
    let len = match list {
        Obj::List(storage) => storage.len(),
        Obj::Bytes(bytes) => bytes.len(),
        _ => return Err(err_msg("Expected List or Bytes in indexing")),
    };
    if idx < 0 || (idx as usize) >= len {
        if !is_index_err {
            return Ok(Obj::Empty);
        }
        let mut rec = StructData::new(SCHEMA_INDEX_ERROR, 2, 1);
        rec.inline_scalars[0] = idx as u64;
        rec.inline_scalars[1] = len as u64;
        rec.inline_objs[0] = Obj::str(format!(
            "Index {} out of bounds for list of length {}",
            idx, len
        ));
        return Ok(Obj::Record(Arc::new(rec)));
    }
    let u = idx as usize;
    Ok(match list {
        Obj::Bytes(bytes) => Obj::boxed_int(i64::from(bytes[u])),
        Obj::List(ListStorage::Int(v)) => {
            if elem_tag == 3 {
                Obj::boxed_bool(v[u] != 0)
            } else {
                Obj::boxed_int(v[u])
            }
        }
        Obj::List(ListStorage::Float(v)) => Obj::boxed_float(v[u].to_bits()),
        Obj::List(ListStorage::Obj(v)) => v[u].clone(),
        _ => unreachable!("list kind was checked above"),
    })
}

fn clamp_slice_bounds(start_i: i64, end_i: i64, len: usize) -> (usize, usize) {
    let start = if start_i <= 0 { 0 } else { (start_i as usize).min(len) };
    let end = if end_i <= 0 { 0 } else { (end_i as usize).min(len) };
    if start >= end { (0, 0) } else { (start, end) }
}

fn list_slice_val(list: &Obj, start_i: i64, end_i: i64) -> Result<Obj, RuntimeError> {
    if let Obj::Bytes(bytes) = list {
        let (start, end) = clamp_slice_bounds(start_i, end_i, bytes.len());
        return Ok(Obj::Bytes(Arc::new(bytes[start..end].to_vec())));
    }
    let Obj::List(storage) = list else {
        return Err(err_msg("Expected List or Bytes in ListSlice"));
    };
    let (s, e) = clamp_slice_bounds(start_i, end_i, storage.len());
    Ok(Obj::List(match storage {
        ListStorage::Int(v) => ListStorage::Int(Arc::new(v[s..e].to_vec())),
        ListStorage::Float(v) => ListStorage::Float(Arc::new(v[s..e].to_vec())),
        ListStorage::Obj(v) => ListStorage::Obj(Arc::new(v[s..e].to_vec())),
    }))
}

fn list_reverse_val(list: &Obj) -> Result<Obj, RuntimeError> {
    let Obj::List(storage) = list else {
        return Err(err_msg("Expected List in ListReverse"));
    };
    Ok(Obj::List(match storage {
        ListStorage::Int(v) => {
            let mut out = v.as_ref().clone();
            out.reverse();
            ListStorage::Int(Arc::new(out))
        }
        ListStorage::Float(v) => {
            let mut out = v.as_ref().clone();
            out.reverse();
            ListStorage::Float(Arc::new(out))
        }
        ListStorage::Obj(v) => {
            let mut out = v.as_ref().clone();
            out.reverse();
            ListStorage::Obj(Arc::new(out))
        }
    }))
}

fn list_enumerate_val(list: &Obj, elem_tag: u8) -> Result<Obj, RuntimeError> {
    let Obj::List(storage) = list else {
        return Err(err_msg("Expected List in ListEnumerate"));
    };
    let mut out = Vec::with_capacity(storage.len());
    match storage {
        ListStorage::Int(v) => {
            for (i, &item) in v.iter().enumerate() {
                let elem = if elem_tag == 3 { Obj::boxed_bool(item != 0) } else { Obj::boxed_int(item) };
                out.push(Obj::List(ListStorage::Obj(Arc::new(vec![Obj::boxed_int(i as i64), elem]))));
            }
        }
        ListStorage::Float(v) => {
            for (i, &item) in v.iter().enumerate() {
                out.push(Obj::List(ListStorage::Obj(Arc::new(vec![
                    Obj::boxed_int(i as i64),
                    Obj::boxed_float(item.to_bits()),
                ]))));
            }
        }
        ListStorage::Obj(v) => {
            for (i, item) in v.iter().enumerate() {
                out.push(Obj::List(ListStorage::Obj(Arc::new(vec![
                    Obj::boxed_int(i as i64),
                    item.clone(),
                ]))));
            }
        }
    }
    Ok(Obj::List(ListStorage::Obj(Arc::new(out))))
}
