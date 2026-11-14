use std::sync::Arc;

use crate::vm::exec_cold::err_msg;
use crate::vm::opcode::{
    EXT_FLOAT_CEIL, EXT_MAP_CONTAINS_KEY, EXT_MAP_ENTRIES, EXT_MAP_FROM, EXT_MAP_GET,
    EXT_MAP_INDEX, EXT_MAP_KEYS, EXT_MAP_REMOVE, EXT_MAP_SET, EXT_MAP_VALUES, EXT_PRINT_OBJ,
    EXT_PRINT_SCALAR, EXT_STR_TO_FLOAT, EXT_STR_TO_INT,
};
use crate::vm::value::{
    ListStorage, MapKey, MapVal, Obj, OrderedMap, SCHEMA_BOXED_BOOL, SCHEMA_BOXED_FLOAT,
    SCHEMA_BOXED_INT, SCHEMA_KEY_ERROR, SCHEMA_PARSE_ERROR, StructData, format_float,
};
use crate::vm::{RuntimeError, Vm};

impl Vm {
    #[cold]
    #[inline(never)]
    pub(crate) fn exec_ext_dispatch(
        &mut self,
        s_base: usize,
        o_base: usize,
        kind: u8,
        aux: u8,
        a: u16,
        b: u16,
        c: u16,
    ) -> Result<(), RuntimeError> {
        match kind {
            EXT_PRINT_SCALAR => {
                let raw = self.scalars[s_base + a as usize];
                let text = match aux {
                    2 => format_float(f64::from_bits(raw)),
                    3 => (raw != 0).to_string(),
                    _ => (raw as i64).to_string(),
                };
                println!("{}", text);
            }
            EXT_PRINT_OBJ => {
                println!("{}", self.objs[o_base + a as usize].format_obj());
            }
            EXT_FLOAT_CEIL => {
                let v = f64::from_bits(self.scalars[s_base + b as usize]).ceil();
                self.scalars[s_base + a as usize] = v.to_bits();
            }
            EXT_STR_TO_INT => {
                let Obj::Str(s) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected String in to_int()"));
                };
                let res = match s.parse::<i64>() {
                    Ok(n) => Obj::boxed_int(n),
                    Err(e) => make_parse_error(Arc::clone(s), format!("Invalid Int '{}': {}", s, e)),
                };
                self.objs[o_base + a as usize] = res;
            }
            EXT_STR_TO_FLOAT => {
                let Obj::Str(s) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected String in to_float()"));
                };
                let res = match s.parse::<f64>() {
                    Ok(f) if f.is_finite() => Obj::boxed_float(f.to_bits()),
                    Ok(_) => make_parse_error(
                        Arc::clone(s),
                        format!("Invalid Float '{}': non-finite value is not allowed", s),
                    ),
                    Err(e) => {
                        make_parse_error(Arc::clone(s), format!("Invalid Float '{}': {}", s, e))
                    }
                };
                self.objs[o_base + a as usize] = res;
            }
            EXT_MAP_SET => {
                let key = self.extract_map_key(s_base, o_base, aux >> 4, b)?;
                let val = match aux & 0x0F {
                    1 => MapVal::Scalar(self.scalars[s_base + c as usize]),
                    2 => MapVal::Float(f64::from_bits(self.scalars[s_base + c as usize])),
                    3 => MapVal::Bool(self.scalars[s_base + c as usize] != 0),
                    _ => MapVal::Obj(self.objs[o_base + c as usize].clone()),
                };
                let Obj::Map(m) = &mut self.objs[o_base + a as usize] else {
                    return Err(err_msg("Expected Map in map.set()"));
                };
                Arc::make_mut(m).set(key, val);
            }
            EXT_MAP_GET | EXT_MAP_INDEX => {
                let v_tag = aux & 0x0F;
                let key = self.extract_map_key(s_base, o_base, aux >> 4, c)?;
                let Obj::Map(m) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected Map in map lookup"));
                };
                let res = match m.get(&key) {
                    Some(mv) => map_val_to_obj(mv, v_tag),
                    None if kind == EXT_MAP_INDEX => make_key_error(&key),
                    None => Obj::Empty,
                };
                self.objs[o_base + a as usize] = res;
            }
            EXT_MAP_CONTAINS_KEY => {
                let key = self.extract_map_key(s_base, o_base, aux, c)?;
                let Obj::Map(m) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected Map in contains_key()"));
                };
                self.scalars[s_base + a as usize] = u64::from(m.get(&key).is_some());
            }
            EXT_MAP_REMOVE => {
                let v_tag = aux & 0x0F;
                let key = self.extract_map_key(s_base, o_base, aux >> 4, c)?;
                let Obj::Map(m) = &mut self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected Map in remove()"));
                };
                let removed = ordered_map_remove(Arc::make_mut(m), &key);
                self.objs[o_base + a as usize] =
                    removed.map(|mv| map_val_to_obj(&mv, v_tag)).unwrap_or(Obj::Empty);
            }
            EXT_MAP_KEYS => {
                let Obj::Map(m) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected Map in keys()"));
                };
                let storage = match aux {
                    1 => ListStorage::Int(Arc::new(
                        m.entries.iter().map(|(k, _)| match k { MapKey::Int(i) => *i, _ => 0 }).collect(),
                    )),
                    3 => ListStorage::Int(Arc::new(
                        m.entries.iter().map(|(k, _)| match k { MapKey::Bool(bl) => i64::from(*bl), _ => 0 }).collect(),
                    )),
                    _ => ListStorage::Obj(Arc::new(
                        m.entries.iter().map(|(k, _)| map_key_to_obj(k)).collect(),
                    )),
                };
                self.objs[o_base + a as usize] = Obj::List(storage);
            }
            EXT_MAP_VALUES => {
                let Obj::Map(m) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected Map in values()"));
                };
                let storage = match aux {
                    1 => ListStorage::Int(Arc::new(
                        m.entries.iter().map(|(_, v)| map_val_to_i64(v)).collect(),
                    )),
                    2 => ListStorage::Float(Arc::new(
                        m.entries.iter().map(|(_, v)| map_val_to_f64(v)).collect(),
                    )),
                    3 => ListStorage::Int(Arc::new(
                        m.entries.iter().map(|(_, v)| i64::from(map_val_to_bool(v))).collect(),
                    )),
                    _ => ListStorage::Obj(Arc::new(
                        m.entries.iter().map(|(_, v)| map_val_to_obj(v, aux)).collect(),
                    )),
                };
                self.objs[o_base + a as usize] = Obj::List(storage);
            }
            EXT_MAP_ENTRIES => {
                let v_tag = aux & 0x0F;
                let Obj::Map(m) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected Map in entries()"));
                };
                let pairs: Vec<Obj> = m
                    .entries
                    .iter()
                    .map(|(k, v)| {
                        Obj::List(ListStorage::Obj(Arc::new(vec![
                            map_key_to_obj(k),
                            map_val_to_obj(v, v_tag),
                        ])))
                    })
                    .collect();
                self.objs[o_base + a as usize] = Obj::List(ListStorage::Obj(Arc::new(pairs)));
            }
            EXT_MAP_FROM => {
                let k_tag = aux >> 4;
                let v_tag = aux & 0x0F;
                let Obj::List(ListStorage::Obj(items)) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected List of tuples in Map.from()"));
                };
                let mut map = OrderedMap::with_capacity(items.len());
                for item in items.iter() {
                    let Obj::List(ListStorage::Obj(pair)) = item else {
                        return Err(err_msg("Expected tuple element in Map.from()"));
                    };
                    if pair.len() != 2 {
                        return Err(err_msg("Expected 2-tuple element in Map.from()"));
                    }
                    let key = obj_to_map_key(&pair[0], k_tag)?;
                    let val = obj_to_map_val(&pair[1], v_tag)?;
                    map.set(key, val);
                }
                self.objs[o_base + a as usize] = Obj::Map(Arc::new(map));
            }
            _ => return Err(err_msg(&format!("Invalid ExtOp kind {}", kind))),
        }
        Ok(())
    }

    fn extract_map_key(
        &self,
        s_base: usize,
        o_base: usize,
        k_tag: u8,
        reg: u16,
    ) -> Result<MapKey, RuntimeError> {
        match k_tag {
            1 => Ok(MapKey::Int(self.scalars[s_base + reg as usize] as i64)),
            3 => Ok(MapKey::Bool(self.scalars[s_base + reg as usize] != 0)),
            0 => match &self.objs[o_base + reg as usize] {
                Obj::Str(s) => Ok(MapKey::Str(Arc::clone(s))),
                _ => Err(err_msg("Expected String map key")),
            },
            _ => Err(err_msg("Unsupported map key type")),
        }
    }
}

fn make_parse_error(value: Arc<String>, message: String) -> Obj {
    let mut rec = StructData::new(SCHEMA_PARSE_ERROR, 0, 2);
    rec.inline_objs[0] = Obj::Str(value);
    rec.inline_objs[1] = Obj::str(message);
    Obj::Record(Arc::new(rec))
}

fn make_key_error(key: &MapKey) -> Obj {
    let ks = match key {
        MapKey::Int(i) => i.to_string(),
        MapKey::Bool(b) => b.to_string(),
        MapKey::Str(s) => s.to_string(),
    };
    let mut rec = StructData::new(SCHEMA_KEY_ERROR, 0, 2);
    rec.inline_objs[1] = Obj::str(format!("Key '{}' not found in map", ks));
    rec.inline_objs[0] = Obj::str(ks);
    Obj::Record(Arc::new(rec))
}

fn ordered_map_remove(m: &mut OrderedMap, k: &MapKey) -> Option<MapVal> {
    if m.dense_ints {
        match k {
            MapKey::Int(idx) if *idx >= 0 && (*idx as usize) < m.entries.len() => {
                m.materialize_index();
            }
            _ => return None,
        }
    }
    let idx = m.index.remove(k)?;
    let (_, val) = m.entries.remove(idx);
    for pos in m.index.values_mut() {
        if *pos > idx {
            *pos -= 1;
        }
    }
    Some(val)
}

fn map_key_to_obj(k: &MapKey) -> Obj {
    match k {
        MapKey::Int(i) => Obj::boxed_int(*i),
        MapKey::Bool(b) => Obj::boxed_bool(*b),
        MapKey::Str(s) => Obj::Str(Arc::clone(s)),
    }
}

fn map_val_to_obj(v: &MapVal, v_tag: u8) -> Obj {
    match v {
        MapVal::Scalar(bits) => match v_tag {
            2 => Obj::boxed_float(*bits),
            3 => Obj::boxed_bool(*bits != 0),
            _ => Obj::boxed_int(*bits as i64),
        },
        MapVal::Float(f) => Obj::boxed_float(f.to_bits()),
        MapVal::Bool(b) => Obj::boxed_bool(*b),
        MapVal::Obj(o) => o.clone(),
    }
}

fn map_val_to_i64(v: &MapVal) -> i64 {
    match v {
        MapVal::Scalar(bits) => *bits as i64,
        MapVal::Obj(Obj::Record(r)) if r.schema_id == SCHEMA_BOXED_INT => r.inline_scalars[0] as i64,
        _ => 0,
    }
}

fn map_val_to_f64(v: &MapVal) -> f64 {
    match v {
        MapVal::Float(f) => *f,
        MapVal::Scalar(bits) => f64::from_bits(*bits),
        MapVal::Obj(Obj::Record(r)) if r.schema_id == SCHEMA_BOXED_FLOAT => {
            f64::from_bits(r.inline_scalars[0])
        }
        _ => 0.0,
    }
}

fn map_val_to_bool(v: &MapVal) -> bool {
    match v {
        MapVal::Bool(b) => *b,
        MapVal::Scalar(bits) => *bits != 0,
        MapVal::Obj(Obj::Record(r)) if r.schema_id == SCHEMA_BOXED_BOOL => r.inline_scalars[0] != 0,
        _ => false,
    }
}

fn obj_to_map_key(obj: &Obj, k_tag: u8) -> Result<MapKey, RuntimeError> {
    match (k_tag, obj) {
        (1, Obj::Record(r)) if r.schema_id == SCHEMA_BOXED_INT => {
            Ok(MapKey::Int(r.inline_scalars[0] as i64))
        }
        (3, Obj::Record(r)) if r.schema_id == SCHEMA_BOXED_BOOL => {
            Ok(MapKey::Bool(r.inline_scalars[0] != 0))
        }
        (0, Obj::Str(s)) => Ok(MapKey::Str(Arc::clone(s))),
        _ => Err(err_msg("Map.from() key type mismatch at runtime")),
    }
}

fn obj_to_map_val(obj: &Obj, v_tag: u8) -> Result<MapVal, RuntimeError> {
    match v_tag {
        1 => match obj {
            Obj::Record(r) if r.schema_id == SCHEMA_BOXED_INT => Ok(MapVal::Scalar(r.inline_scalars[0])),
            _ => Err(err_msg("Map.from() expected Int value")),
        },
        2 => match obj {
            Obj::Record(r) if r.schema_id == SCHEMA_BOXED_FLOAT => {
                Ok(MapVal::Float(f64::from_bits(r.inline_scalars[0])))
            }
            _ => Err(err_msg("Map.from() expected Float value")),
        },
        3 => match obj {
            Obj::Record(r) if r.schema_id == SCHEMA_BOXED_BOOL => {
                Ok(MapVal::Bool(r.inline_scalars[0] != 0))
            }
            _ => Err(err_msg("Map.from() expected Bool value")),
        },
        _ => Ok(MapVal::Obj(obj.clone())),
    }
}
