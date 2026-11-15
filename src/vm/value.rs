use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::sync::Arc;

/// Fast non-cryptographic 64-bit hasher for internal maps.
#[derive(Clone, Default)]
pub struct FxBuildHasher;

pub struct FxHasher {
    hash: u64,
}

impl BuildHasher for FxBuildHasher {
    type Hasher = FxHasher;

    #[inline]
    fn build_hasher(&self) -> FxHasher {
        FxHasher { hash: 0 }
    }
}

impl FxHasher {
    const K: u64 = 0x517cc1b727220a95;

    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(Self::K);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }

    #[inline]
    fn write(&mut self, mut bytes: &[u8]) {
        while bytes.len() >= 8 {
            let chunk = u64::from_ne_bytes(bytes[..8].try_into().unwrap());
            self.add(chunk);
            bytes = &bytes[8..];
        }
        if bytes.len() >= 4 {
            let chunk = u32::from_ne_bytes(bytes[..4].try_into().unwrap()) as u64;
            self.add(chunk);
            bytes = &bytes[4..];
        }
        if bytes.len() >= 2 {
            let chunk = u16::from_ne_bytes(bytes[..2].try_into().unwrap()) as u64;
            self.add(chunk);
            bytes = &bytes[2..];
        }
        if let Some(&b) = bytes.first() {
            self.add(b as u64);
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(i as u64);
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }

    #[inline]
    fn write_i64(&mut self, i: i64) {
        self.add(i as u64);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }
}

/// Specialized list storage avoiding Value boxing for numeric lists.
#[derive(Debug, Clone)]
pub enum ListStorage {
    Int(Arc<Vec<i64>>),
    Float(Arc<Vec<f64>>),
    Obj(Arc<Vec<Obj>>),
}

impl ListStorage {
    #[inline]
    pub fn new_int(cap: usize) -> Self {
        Self::Int(Arc::new(Vec::with_capacity(cap)))
    }

    #[inline]
    pub fn new_float(cap: usize) -> Self {
        Self::Float(Arc::new(Vec::with_capacity(cap)))
    }

    #[inline]
    pub fn new_obj(cap: usize) -> Self {
        Self::Obj(Arc::new(Vec::with_capacity(cap)))
    }

    #[inline]
    pub fn len(&self) -> usize {
        match self {
            ListStorage::Int(v) => v.len(),
            ListStorage::Float(v) => v.len(),
            ListStorage::Obj(v) => v.len(),
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn append_int(&mut self, val: i64) {
        match self {
            ListStorage::Int(arc) => {
                if let Some(vec) = Arc::get_mut(arc) {
                    vec.push(val);
                } else {
                    append_int_cow(arc, val);
                }
            }
            _ => panic!("append_int called on non-Int ListStorage"),
        }
    }

    #[inline]
    pub fn append_float(&mut self, val: f64) {
        match self {
            ListStorage::Float(arc) => {
                if let Some(vec) = Arc::get_mut(arc) {
                    vec.push(val);
                } else {
                    append_float_cow(arc, val);
                }
            }
            _ => panic!("append_float called on non-Float ListStorage"),
        }
    }

    #[inline]
    pub fn append_obj(&mut self, val: Obj) {
        match self {
            ListStorage::Obj(arc) => {
                if let Some(vec) = Arc::get_mut(arc) {
                    vec.push(val);
                } else {
                    append_obj_cow(arc, val);
                }
            }
            _ => panic!("append_obj called on non-Obj ListStorage"),
        }
    }
}

#[cold]
#[inline(never)]
pub(crate) fn append_int_cow(arc: &mut Arc<Vec<i64>>, val: i64) {
    Arc::make_mut(arc).push(val);
}

#[cold]
#[inline(never)]
pub(crate) fn append_float_cow(arc: &mut Arc<Vec<f64>>, val: f64) {
    Arc::make_mut(arc).push(val);
}

#[cold]
#[inline(never)]
pub(crate) fn append_obj_cow(arc: &mut Arc<Vec<Obj>>, val: Obj) {
    Arc::make_mut(arc).push(val);
}

impl PartialEq for ListStorage {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ListStorage::Int(a), ListStorage::Int(b)) => {
                Arc::ptr_eq(a, b) || a.as_slice() == b.as_slice()
            }
            (ListStorage::Float(a), ListStorage::Float(b)) => {
                Arc::ptr_eq(a, b) || a.as_slice() == b.as_slice()
            }
            (ListStorage::Obj(a), ListStorage::Obj(b)) => {
                Arc::ptr_eq(a, b) || a.as_slice() == b.as_slice()
            }
            _ => false,
        }
    }
}

pub const INLINE_STRUCT_SCALARS: usize = 2;
pub const INLINE_STRUCT_OBJS: usize = 2;
pub const OVERFLOW_INLINE_SCALARS: usize = 2;
pub const OVERFLOW_INLINE_OBJS: usize = 4;

pub const SCHEMA_EMPTY_TAG: u32 = 0xFFFF_0000;
pub const SCHEMA_BOXED_INT: u32 = 0xFFFF_0001;
pub const SCHEMA_BOXED_FLOAT: u32 = 0xFFFF_0002;
pub const SCHEMA_BOXED_BOOL: u32 = 0xFFFF_0003;
pub const SCHEMA_STR_TAG: u32 = 0xFFFF_0004;
pub const SCHEMA_LIST_TAG: u32 = 0xFFFF_0005;
pub const SCHEMA_MAP_TAG: u32 = 0xFFFF_0006;

pub const SCHEMA_TIME_TIMESTAMP: u32 = 0xFFFF_0010;
pub const SCHEMA_TIME_INSTANT: u32 = 0xFFFF_0011;
pub const SCHEMA_TIME_DURATION: u32 = 0xFFFF_0012;
pub const SCHEMA_TIME_ERROR: u32 = 0xFFFF_0013;

pub const SCHEMA_VALIDATION_ISSUE: u32 = 0xFFFF_0020;
pub const SCHEMA_VALIDATION_VIOLATION: u32 = 0xFFFF_0021;
pub const SCHEMA_VALIDATION_ERROR: u32 = 0xFFFF_0022;

pub const SCHEMA_JSON_DECODE_ERROR: u32 = 0xFFFF_0030;
pub const SCHEMA_JSON_ENCODE_ERROR: u32 = 0xFFFF_0031;
pub const SCHEMA_INDEX_ERROR: u32 = 0xFFFF_0032;
pub const SCHEMA_KEY_ERROR: u32 = 0xFFFF_0033;
pub const SCHEMA_PARSE_ERROR: u32 = 0xFFFF_0034;
pub const SCHEMA_ENV_ERROR: u32 = 0xFFFF_0035;

#[inline]
pub fn is_builtin_error_schema(schema_id: u32) -> bool {
    matches!(
        schema_id,
        SCHEMA_TIME_ERROR
            | SCHEMA_VALIDATION_ISSUE
            | SCHEMA_VALIDATION_ERROR
            | SCHEMA_JSON_DECODE_ERROR
            | SCHEMA_JSON_ENCODE_ERROR
            | SCHEMA_INDEX_ERROR
            | SCHEMA_KEY_ERROR
            | SCHEMA_PARSE_ERROR
            | SCHEMA_ENV_ERROR
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructOverflow {
    pub inline_scalars: [u64; OVERFLOW_INLINE_SCALARS],
    pub inline_objs: [Obj; OVERFLOW_INLINE_OBJS],
    pub extra_scalars: Box<[u64]>,
    pub extra_objs: Box<[Obj]>,
}

/// Fixed-slot record representation with inline small-struct optimization (64 bytes).
/// Structs with <= 2 scalar fields and <= 2 object fields require no heap overflow box.
#[derive(Debug, Clone)]
pub struct StructData {
    pub schema_id: u32,
    pub scalar_len: u16,
    pub obj_len: u16,
    pub overflow: Option<Box<StructOverflow>>,
    pub inline_scalars: [u64; INLINE_STRUCT_SCALARS],
    pub inline_objs: [Obj; INLINE_STRUCT_OBJS],
}

impl StructData {
    #[inline]
    pub fn new(schema_id: u32, scalar_len: u16, obj_len: u16) -> Self {
        let s_len = scalar_len as usize;
        let o_len = obj_len as usize;
        let overflow = if s_len > INLINE_STRUCT_SCALARS || o_len > INLINE_STRUCT_OBJS {
            let max_inline_s = INLINE_STRUCT_SCALARS + OVERFLOW_INLINE_SCALARS;
            let max_inline_o = INLINE_STRUCT_OBJS + OVERFLOW_INLINE_OBJS;
            let extra_scalars = if s_len > max_inline_s {
                vec![0u64; s_len - max_inline_s].into_boxed_slice()
            } else {
                Box::new([])
            };
            let extra_objs = if o_len > max_inline_o {
                vec![Obj::Empty; o_len - max_inline_o].into_boxed_slice()
            } else {
                Box::new([])
            };
            Some(Box::new(StructOverflow {
                inline_scalars: [0; OVERFLOW_INLINE_SCALARS],
                inline_objs: [const { Obj::Empty }; OVERFLOW_INLINE_OBJS],
                extra_scalars,
                extra_objs,
            }))
        } else {
            None
        };
        Self {
            schema_id,
            scalar_len,
            obj_len,
            overflow,
            inline_scalars: [0; INLINE_STRUCT_SCALARS],
            inline_objs: [const { Obj::Empty }; INLINE_STRUCT_OBJS],
        }
    }

    #[inline(always)]
    pub fn get_scalar(&self, idx: usize) -> u64 {
        if idx < INLINE_STRUCT_SCALARS {
            self.inline_scalars[idx]
        } else {
            let ov = self.overflow.as_ref().unwrap();
            let ov_idx = idx - INLINE_STRUCT_SCALARS;
            if ov_idx < OVERFLOW_INLINE_SCALARS {
                ov.inline_scalars[ov_idx]
            } else {
                ov.extra_scalars[ov_idx - OVERFLOW_INLINE_SCALARS]
            }
        }
    }

    #[inline(always)]
    pub fn set_scalar(&mut self, idx: usize, val: u64) {
        if idx < INLINE_STRUCT_SCALARS {
            self.inline_scalars[idx] = val;
        } else {
            let ov = self.overflow.as_mut().unwrap();
            let ov_idx = idx - INLINE_STRUCT_SCALARS;
            if ov_idx < OVERFLOW_INLINE_SCALARS {
                ov.inline_scalars[ov_idx] = val;
            } else {
                ov.extra_scalars[ov_idx - OVERFLOW_INLINE_SCALARS] = val;
            }
        }
    }

    #[inline(always)]
    pub fn get_obj(&self, idx: usize) -> &Obj {
        if idx < INLINE_STRUCT_OBJS {
            &self.inline_objs[idx]
        } else {
            let ov = self.overflow.as_ref().unwrap();
            let ov_idx = idx - INLINE_STRUCT_OBJS;
            if ov_idx < OVERFLOW_INLINE_OBJS {
                &ov.inline_objs[ov_idx]
            } else {
                &ov.extra_objs[ov_idx - OVERFLOW_INLINE_OBJS]
            }
        }
    }

    #[inline(always)]
    pub fn set_obj(&mut self, idx: usize, val: Obj) {
        if idx < INLINE_STRUCT_OBJS {
            self.inline_objs[idx] = val;
        } else {
            let ov = self.overflow.as_mut().unwrap();
            let ov_idx = idx - INLINE_STRUCT_OBJS;
            if ov_idx < OVERFLOW_INLINE_OBJS {
                ov.inline_objs[ov_idx] = val;
            } else {
                ov.extra_objs[ov_idx - OVERFLOW_INLINE_OBJS] = val;
            }
        }
    }
}

impl PartialEq for StructData {
    fn eq(&self, other: &Self) -> bool {
        if self.schema_id != other.schema_id
            || self.scalar_len != other.scalar_len
            || self.obj_len != other.obj_len
        {
            return false;
        }
        let s_len = self.scalar_len as usize;
        let inline_s = s_len.min(INLINE_STRUCT_SCALARS);
        if self.inline_scalars[..inline_s] != other.inline_scalars[..inline_s] {
            return false;
        }
        let o_len = self.obj_len as usize;
        let inline_o = o_len.min(INLINE_STRUCT_OBJS);
        if self.inline_objs[..inline_o] != other.inline_objs[..inline_o] {
            return false;
        }
        if (s_len > INLINE_STRUCT_SCALARS || o_len > INLINE_STRUCT_OBJS)
            && self.overflow != other.overflow
        {
            return false;
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MapKey {
    Int(i64),
    Bool(bool),
    Str(Arc<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum MapVal {
    Scalar(u64),
    Float(f64),
    Bool(bool),
    Obj(Obj),
}

/// Insertion-ordered dictionary preserving Conduit's Map<K, V> semantics with FxHash and dense-int fast path.
#[derive(Debug, Clone)]
pub struct OrderedMap {
    pub(crate) entries: Vec<(MapKey, MapVal)>,
    pub(crate) index: HashMap<MapKey, usize, FxBuildHasher>,
    pub(crate) dense_ints: bool,
}

impl OrderedMap {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            index: HashMap::with_hasher(FxBuildHasher),
            dense_ints: true,
        }
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            entries: Vec::with_capacity(cap),
            index: HashMap::with_capacity_and_hasher(cap, FxBuildHasher),
            dense_ints: true,
        }
    }

    #[inline]
    pub fn entries(&self) -> &[(MapKey, MapVal)] {
        &self.entries
    }

    #[cold]
    #[inline(never)]
    pub(crate) fn materialize_index(&mut self) {
        self.dense_ints = false;
        if !self.entries.is_empty() {
            self.index.reserve(self.entries.len());
            for (pos, (key, _)) in self.entries.iter().enumerate() {
                self.index.insert(key.clone(), pos);
            }
        }
    }

    #[inline]
    pub fn set(&mut self, key: MapKey, val: MapVal) {
        if self.dense_ints {
            if let MapKey::Int(k) = key
                && k >= 0
            {
                let idx = k as usize;
                if idx < self.entries.len() {
                    self.entries[idx].1 = val;
                    return;
                }
                if idx == self.entries.len() {
                    self.entries.push((MapKey::Int(k), val));
                    return;
                }
            }
            self.materialize_index();
        }
        match self.index.entry(key) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                self.entries[*entry.get()].1 = val;
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                let pos = self.entries.len();
                let key = entry.key().clone();
                entry.insert(pos);
                self.entries.push((key, val));
            }
        }
    }

    #[inline]
    pub fn get(&self, key: &MapKey) -> Option<&MapVal> {
        if self.dense_ints {
            return match key {
                MapKey::Int(k) if *k >= 0 => self.entries.get(*k as usize).map(|(_, v)| v),
                _ => None,
            };
        }
        self.index.get(key).map(|&pos| &self.entries[pos].1)
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for OrderedMap {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for OrderedMap {
    fn eq(&self, other: &Self) -> bool {
        if self.entries.len() != other.entries.len() {
            return false;
        }
        self.entries
            .iter()
            .all(|(k, v)| other.get(k) == Some(v))
    }
}

pub const INLINE_CLOSURE_SCALARS: usize = 4;

/// Closure value capturing only referenced free variables into flat arrays.
#[derive(Debug, Clone)]
pub struct ClosureData {
    pub func_id: u32,
    pub scalar_len: u16,
    pub inline_scalars: [u64; INLINE_CLOSURE_SCALARS],
    pub extra_scalars: Option<Box<[u64]>>,
    pub obj_upvalues: Box<[Obj]>,
}

impl ClosureData {
    #[inline]
    pub fn from_captures(
        func_id: u32,
        scalar_vals: &[u64],
        obj_vals: Vec<Obj>,
    ) -> Self {
        let mut inline_scalars = [0u64; INLINE_CLOSURE_SCALARS];
        let copy_len = scalar_vals.len().min(INLINE_CLOSURE_SCALARS);
        inline_scalars[..copy_len].copy_from_slice(&scalar_vals[..copy_len]);
        let extra_scalars = if scalar_vals.len() > INLINE_CLOSURE_SCALARS {
            Some(scalar_vals[INLINE_CLOSURE_SCALARS..].to_vec().into_boxed_slice())
        } else {
            None
        };
        Self {
            func_id,
            scalar_len: scalar_vals.len() as u16,
            inline_scalars,
            extra_scalars,
            obj_upvalues: obj_vals.into_boxed_slice(),
        }
    }

    #[inline(always)]
    pub fn get_scalar(&self, idx: usize) -> u64 {
        if idx < INLINE_CLOSURE_SCALARS {
            self.inline_scalars[idx]
        } else {
            self.extra_scalars.as_ref().unwrap()[idx - INLINE_CLOSURE_SCALARS]
        }
    }

    #[inline(always)]
    pub fn get_obj(&self, idx: usize) -> &Obj {
        &self.obj_upvalues[idx]
    }
}

impl PartialEq for ClosureData {
    fn eq(&self, other: &Self) -> bool {
        self.func_id == other.func_id
            && self.scalar_len == other.scalar_len
            && self.inline_scalars == other.inline_scalars
            && self.extra_scalars == other.extra_scalars
            && self.obj_upvalues == other.obj_upvalues
    }
}

/// Heap-allocated or reference-counted object register value (<= 16 bytes).
#[derive(Debug, Clone, Default)]
pub enum Obj {
    #[default]
    Empty,
    Str(Arc<String>),
    List(ListStorage),
    Record(Arc<StructData>),
    Map(Arc<OrderedMap>),
    Closure(Arc<ClosureData>),
    Func(u32),
}

impl Obj {
    #[inline]
    pub fn str(s: impl Into<String>) -> Self {
        Obj::Str(Arc::new(s.into()))
    }

    #[inline]
    pub fn boxed_int(v: i64) -> Self {
        let mut s = StructData::new(SCHEMA_BOXED_INT, 1, 0);
        s.inline_scalars[0] = v as u64;
        Obj::Record(Arc::new(s))
    }

    #[inline]
    pub fn boxed_float(bits: u64) -> Self {
        let mut s = StructData::new(SCHEMA_BOXED_FLOAT, 1, 0);
        s.inline_scalars[0] = bits;
        Obj::Record(Arc::new(s))
    }

    #[inline]
    pub fn boxed_bool(b: bool) -> Self {
        let mut s = StructData::new(SCHEMA_BOXED_BOOL, 1, 0);
        s.inline_scalars[0] = b as u64;
        Obj::Record(Arc::new(s))
    }

    pub fn format_obj(&self) -> String {
        match self {
            Obj::Empty => "nil".to_string(),
            Obj::Str(s) => s.as_ref().clone(),
            Obj::Record(rec) => match rec.schema_id {
                SCHEMA_BOXED_INT => (rec.inline_scalars[0] as i64).to_string(),
                SCHEMA_BOXED_FLOAT => format_float(f64::from_bits(rec.inline_scalars[0])),
                SCHEMA_BOXED_BOOL => (rec.inline_scalars[0] != 0).to_string(),
                SCHEMA_KEY_ERROR | SCHEMA_PARSE_ERROR | SCHEMA_ENV_ERROR if rec.inline_objs.len() >= 2 => {
                    rec.inline_objs[1].format_obj()
                }
                _ if !rec.inline_objs.is_empty() && matches!(&rec.inline_objs[0], Obj::Str(_)) => {
                    rec.inline_objs[0].format_obj()
                }
                _ => "<record>".to_string(),
            },
            Obj::List(ListStorage::Int(v)) => {
                let parts: Vec<String> = v.iter().map(|x| x.to_string()).collect();
                format!("[{}]", parts.join(", "))
            }
            Obj::List(ListStorage::Float(v)) => {
                let parts: Vec<String> = v.iter().map(|x| format_float(*x)).collect();
                format!("[{}]", parts.join(", "))
            }
            Obj::List(ListStorage::Obj(v)) => {
                let parts: Vec<String> = v.iter().map(|x| x.format_obj()).collect();
                format!("[{}]", parts.join(", "))
            }
            Obj::Map(_) => "<map>".to_string(),
            Obj::Closure(_) | Obj::Func(_) => "<func>".to_string(),
        }
    }
}

#[inline]
pub fn format_float(v: f64) -> String {
    format!("{}", v)
}

impl PartialEq for Obj {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Obj::Empty, Obj::Empty) => true,
            (Obj::Str(a), Obj::Str(b)) => Arc::ptr_eq(a, b) || a.as_str() == b.as_str(),
            (Obj::List(a), Obj::List(b)) => a == b,
            (Obj::Record(a), Obj::Record(b)) => Arc::ptr_eq(a, b) || a.as_ref() == b.as_ref(),
            (Obj::Map(a), Obj::Map(b)) => Arc::ptr_eq(a, b) || a.as_ref() == b.as_ref(),
            (Obj::Closure(a), Obj::Closure(b)) => Arc::ptr_eq(a, b) || a.as_ref() == b.as_ref(),
            (Obj::Func(a), Obj::Func(b)) => a == b,
            _ => false,
        }
    }
}

#[cfg(test)]
mod layout_tests {
    use super::Obj;

    #[test]
    fn obj_size_is_16_bytes() {
        assert!(
            std::mem::size_of::<Obj>() <= 16,
            "Obj size was {} bytes",
            std::mem::size_of::<Obj>()
        );
    }
}
