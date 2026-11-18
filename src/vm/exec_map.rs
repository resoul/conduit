use std::cell::RefCell;
use std::io::{BufWriter, IsTerminal, Write};
use std::sync::{Arc, OnceLock};

const STDOUT_BUF_CAP: usize = 64 * 1024;

thread_local! {
    static STDOUT_BUF: RefCell<Option<BufWriter<std::io::Stdout>>> = const { RefCell::new(None) };
}

#[inline]
fn stdout_is_tty() -> bool {
    static IS_TTY: OnceLock<bool> = OnceLock::new();
    *IS_TTY.get_or_init(|| std::io::stdout().is_terminal())
}

#[inline]
pub(crate) fn print_stdout_line(s: &str) {
    if cfg!(test) || stdout_is_tty() {
        println!("{}", s);
        return;
    }
    STDOUT_BUF.with(|cell| {
        let mut slot = cell.borrow_mut();
        let bw = slot
            .get_or_insert_with(|| BufWriter::with_capacity(STDOUT_BUF_CAP, std::io::stdout()));
        let _ = bw.write_all(s.as_bytes());
        let _ = bw.write_all(b"\n");
    });
}

pub(crate) fn flush_stdout() {
    if cfg!(test) || stdout_is_tty() {
        return;
    }
    STDOUT_BUF.with(|cell| {
        if let Some(bw) = cell.borrow_mut().as_mut() {
            let _ = bw.flush();
        }
    });
}

use crate::vm::exec_cold::{err_msg, err_overflow};
use crate::native::cli::{exec_cli_parse, format_cli_help};
use crate::native::env::{exec_env_load, make_env_error};
use crate::vm::exec_union::list_get_or_index;
use crate::vm::opcode::{
    EXT_CLI_HELP, EXT_CLI_PARSE, EXT_CLI_PARSE_ARGS, EXT_CRYPTO_OP, EXT_ENV_ARGS, EXT_ENV_GET,
    EXT_ENV_LOAD, EXT_ENV_REQUIRE, EXT_FLOAT_CEIL, EXT_FLOAT_MATH, EXT_FS_BINARY, EXT_FS_UNARY,
    EXT_INT_MATH, EXT_LIST_FIRST, EXT_LIST_IS_EMPTY, EXT_LIST_LAST, EXT_LIST_SORT,
    EXT_MAP_CONTAINS_KEY, EXT_MAP_ENTRIES, EXT_MAP_FROM, EXT_MAP_GET, EXT_MAP_INDEX, EXT_MAP_KEYS,
    EXT_MAP_REMOVE, EXT_MAP_SET, EXT_MAP_VALUES, EXT_PATH_OP, EXT_PIPELINE_REDUCE, EXT_PRINT_OBJ,
    EXT_PRINT_SCALAR, EXT_PROCESS_OP, EXT_STR_IS_EMPTY, EXT_STR_PRED, EXT_STR_SPLIT,
    EXT_STR_TO_FLOAT, EXT_STR_TO_INT, EXT_URL_OP, EXT_CSV_OP, EXT_DB_OP, EXT_CRON_OP, EXT_MATH_OP, EXT_LOG_OP,
    EXT_LIST_AGG, EXT_HTTP_OP,
};
use crate::vm::value::{
    ListStorage, MapKey, MapVal, Obj, OrderedMap, SCHEMA_BOXED_BOOL, SCHEMA_BOXED_FLOAT,
    SCHEMA_BOXED_INT, SCHEMA_KEY_ERROR, SCHEMA_PARSE_ERROR, StructData, format_float,
};
use crate::vm::{RuntimeError, Vm};
use crate::vm::string::VmString;

impl Vm {
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_ext_dispatch(
        &mut self,
        module: &crate::ir::Module,
        s_base: usize,
        o_base: usize,
        s_top: usize,
        o_top: usize,
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
                print_stdout_line(&text);
            }
            EXT_PRINT_OBJ => {
                if let Obj::Str(s) = &self.objs[o_base + a as usize] {
                    print_stdout_line(s.as_str());
                } else {
                    print_stdout_line(&self.objs[o_base + a as usize].format_obj());
                }
            }
            EXT_FLOAT_CEIL => {
                let v = f64::from_bits(self.scalars[s_base + b as usize]).ceil();
                self.scalars[s_base + a as usize] = v.to_bits();
            }
            EXT_STR_IS_EMPTY => {
                let Obj::Str(s) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected String in is_empty()"));
                };
                self.scalars[s_base + a as usize] = u64::from(s.is_empty());
            }
            EXT_STR_PRED => {
                let (Obj::Str(s), Obj::Str(arg)) =
                    (&self.objs[o_base + b as usize], &self.objs[o_base + c as usize])
                else {
                    return Err(err_msg("Expected String in string predicate"));
                };
                let matched = match aux {
                    0 => s.contains(arg.as_str()),
                    1 => s.starts_with(arg.as_str()),
                    _ => s.ends_with(arg.as_str()),
                };
                self.scalars[s_base + a as usize] = u64::from(matched);
            }
            EXT_STR_SPLIT => {
                let (Obj::Str(s), Obj::Str(sep)) =
                    (&self.objs[o_base + b as usize], &self.objs[o_base + c as usize])
                else {
                    return Err(err_msg("Expected String in split()"));
                };
                let parts: Vec<Obj> = s
                    .split(sep.as_str())
                    .map(|p| Obj::str(p.to_string()))
                    .collect();
                self.objs[o_base + a as usize] = Obj::List(ListStorage::Obj(Arc::new(parts)));
            }
            EXT_INT_MATH => {
                let lhs = self.scalars[s_base + b as usize] as i64;
                let res = match aux {
                    0 => lhs.checked_abs().ok_or_else(|| err_msg("Integer overflow in abs()"))?,
                    1 => lhs.min(self.scalars[s_base + c as usize] as i64),
                    _ => lhs.max(self.scalars[s_base + c as usize] as i64),
                };
                self.scalars[s_base + a as usize] = res as u64;
            }
            EXT_FLOAT_MATH => {
                let lhs = f64::from_bits(self.scalars[s_base + b as usize]);
                let res = match aux {
                    0 => lhs.abs(),
                    1 => lhs.min(f64::from_bits(self.scalars[s_base + c as usize])),
                    _ => lhs.max(f64::from_bits(self.scalars[s_base + c as usize])),
                };
                self.scalars[s_base + a as usize] = res.to_bits();
            }
            EXT_LIST_IS_EMPTY => {
                let Obj::List(list) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected List in is_empty()"));
                };
                self.scalars[s_base + a as usize] = u64::from(list.is_empty());
            }
            EXT_LIST_FIRST => {
                let res = list_get_or_index(&self.objs[o_base + b as usize], 0, aux, false)?;
                self.objs[o_base + a as usize] = res;
            }
            EXT_LIST_LAST => {
                let obj = &self.objs[o_base + b as usize];
                let Obj::List(list) = obj else {
                    return Err(err_msg("Expected List in last()"));
                };
                let len = list.len();
                let res = if len == 0 {
                    Obj::Empty
                } else {
                    list_get_or_index(obj, (len - 1) as i64, aux, false)?
                };
                self.objs[o_base + a as usize] = res;
            }
            EXT_LIST_SORT => {
                let Obj::List(list) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected List in sort()"));
                };
                let sorted = match (aux, list) {
                    (0, ListStorage::Int(v)) => {
                        let mut out = v.as_ref().clone();
                        out.sort_unstable();
                        Obj::List(ListStorage::Int(Arc::new(out)))
                    }
                    (1, ListStorage::Float(v)) => {
                        let mut out = v.as_ref().clone();
                        out.sort_by(|x, y| x.total_cmp(y));
                        Obj::List(ListStorage::Float(Arc::new(out)))
                    }
                    (2, ListStorage::Obj(v)) => {
                        let mut out = v.as_ref().clone();
                        out.sort_by(|x, y| match (x, y) {
                            (Obj::Str(sa), Obj::Str(sb)) => sa.cmp(sb),
                            _ => std::cmp::Ordering::Equal,
                        });
                        Obj::List(ListStorage::Obj(Arc::new(out)))
                    }
                    _ => return Err(err_msg("Unsupported list storage in sort()")),
                };
                self.objs[o_base + a as usize] = sorted;
            }
            EXT_ENV_GET | EXT_ENV_REQUIRE => {
                let Obj::Str(name) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected String in cdt.env"));
                };
                let res = match std::env::var(name.as_str()) {
                    Ok(v) if kind == EXT_ENV_GET || !v.is_empty() => Obj::str(v),
                    _ if kind == EXT_ENV_GET => Obj::Empty,
                    _ => make_env_error(
                        name.as_str(),
                        format!("Missing required environment variable '{}'", name),
                    ),
                };
                self.objs[o_base + a as usize] = res;
            }
            EXT_ENV_ARGS => {
                let args: Vec<Obj> = std::env::args().map(Obj::str).collect();
                self.objs[o_base + a as usize] = Obj::List(ListStorage::Obj(Arc::new(args)));
            }
            EXT_ENV_LOAD => {
                let plan = module
                    .env_plans
                    .get(b as usize)
                    .ok_or_else(|| err_msg("Missing EnvLoadPlan"))?;
                self.objs[o_base + a as usize] = exec_env_load(plan);
            }
            EXT_CLI_PARSE => {
                let plan = module
                    .cli_plans
                    .get(b as usize)
                    .ok_or_else(|| err_msg("Missing CliPlan"))?;
                let args: Vec<String> = self
                    .script_args
                    .clone()
                    .unwrap_or_else(|| std::env::args().skip(1).collect());
                self.objs[o_base + a as usize] = exec_cli_parse(plan, &args);
            }
            EXT_CLI_PARSE_ARGS => {
                let plan = module
                    .cli_plans
                    .get(b as usize)
                    .ok_or_else(|| err_msg("Missing CliPlan"))?;
                let Obj::List(ListStorage::Obj(items)) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected [String] in cli.parse_args()"));
                };
                let mut args = Vec::with_capacity(items.len());
                for item in items.iter() {
                    let Obj::Str(s) = item else {
                        return Err(err_msg("Expected String element in cli.parse_args()"));
                    };
                    args.push(s.as_str().to_owned());
                }
                self.objs[o_base + a as usize] = exec_cli_parse(plan, &args);
            }
            EXT_CLI_HELP => {
                let plan = module
                    .cli_plans
                    .get(b as usize)
                    .ok_or_else(|| err_msg("Missing CliPlan"))?;
                let root = plan.nodes.first().map(|n| vec![n.name.clone()]).unwrap_or_default();
                self.objs[o_base + a as usize] = Obj::str(format_cli_help(plan, 0, &root));
            }
            EXT_FS_UNARY => {
                self.exec_fs_unary(s_base, o_base, aux, a, b)?;
            }
            EXT_FS_BINARY => {
                self.exec_fs_binary(module, s_base, o_base, s_top, o_top, aux, a, b, c)?;
            }
            EXT_PATH_OP => {
                self.exec_path_op(s_base, o_base, aux, a, b, c)?;
            }
            EXT_PROCESS_OP => {
                self.exec_process_op(s_base, o_base, aux, a, b, c)?;
            }
            EXT_CRYPTO_OP => {
                self.exec_crypto_op(s_base, o_base, aux, a, b, c)?;
            }
            EXT_PIPELINE_REDUCE => {
                self.run_pipeline_reduce(module, s_base, o_base, s_top, o_top, a, b, c)?;
            }
            EXT_URL_OP => {
                self.exec_url_op(o_base, aux, a, b)?;
            }
            EXT_CSV_OP => {
                self.exec_csv_op(o_base, aux, a, b, c)?;
            }
            EXT_MATH_OP => {
                self.exec_math_op(s_base, aux, a, b, c)?;
            }
            EXT_LOG_OP => {
                self.exec_log_op(s_base, o_base, aux, a, b)?;
            }
            EXT_LIST_AGG => {
                let Obj::List(storage) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected List in aggregate method"));
                };
                match (storage, aux) {
                    (ListStorage::Int(v), 0) => {
                        let mut acc: i64 = 0;
                        let mut ov = false;
                        for &x in v.iter() {
                            let (next, o) = acc.overflowing_add(x);
                            acc = next;
                            ov |= o;
                        }
                        if ov {
                            return Err(err_overflow());
                        }
                        self.scalars[s_base + a as usize] = acc as u64;
                    }
                    (ListStorage::Float(v), 0) => {
                        let mut acc = 0.0f64;
                        for &x in v.iter() {
                            acc += x;
                        }
                        self.scalars[s_base + a as usize] = acc.to_bits();
                    }
                    (ListStorage::Int(v), 1) => {
                        self.objs[o_base + a as usize] = match v.iter().copied().min() {
                            Some(m) => Obj::boxed_int(m),
                            None => Obj::Empty,
                        };
                    }
                    (ListStorage::Int(v), 2) => {
                        self.objs[o_base + a as usize] = match v.iter().copied().max() {
                            Some(m) => Obj::boxed_int(m),
                            None => Obj::Empty,
                        };
                    }
                    (ListStorage::Float(v), 1) => {
                        self.objs[o_base + a as usize] = if v.is_empty() {
                            Obj::Empty
                        } else {
                            let mut m = v[0];
                            for &x in &v[1..] {
                                if x < m {
                                    m = x;
                                }
                            }
                            Obj::boxed_float(m.to_bits())
                        };
                    }
                    (ListStorage::Float(v), 2) => {
                        self.objs[o_base + a as usize] = if v.is_empty() {
                            Obj::Empty
                        } else {
                            let mut m = v[0];
                            for &x in &v[1..] {
                                if x > m {
                                    m = x;
                                }
                            }
                            Obj::boxed_float(m.to_bits())
                        };
                    }
                    _ => return Err(err_msg("List aggregate only supports [Int] and [Float]")),
                }
            }
            EXT_HTTP_OP => {
                self.exec_http_op(module, s_base, o_base, s_top, o_top, aux, a, b, c)?;
            }
            EXT_DB_OP => {
                self.exec_db_op(module, s_base, o_base, s_top, o_top, aux, a, b, c)?;
            }
            EXT_CRON_OP => {
                self.exec_cron_op(module, s_base, o_base, aux, a, b, c)?;
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

fn make_parse_error(value: Arc<VmString>, message: String) -> Obj {
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
    if m.index.is_empty() {
        let idx = m.entries.iter().position(|(ek, _)| ek == k)?;
        return Some(m.entries.remove(idx).1);
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
