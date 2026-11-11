use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use crate::ast::{Block, Param};
use crate::native::NativeValue;
use crate::types::TypeId;

#[derive(Debug)]
pub struct StructSchema {
    pub type_id: TypeId,
    names: Box<[Box<str>]>,
    json_names: Box<[Box<str>]>,
    json_prefixes: OnceLock<Box<[String]>>,
    sorted_order: Box<[usize]>,
    index: HashMap<Box<str>, usize>,
}

impl StructSchema {
    pub fn new(type_id: TypeId, field_names: impl IntoIterator<Item = String>) -> Arc<Self> {
        let fields = field_names
            .into_iter()
            .map(|name| {
                let json_name = name.clone();
                (name, json_name)
            })
            .collect();
        Self::with_json_names(type_id, fields)
    }

    pub fn with_json_names(type_id: TypeId, fields: Vec<(String, String)>) -> Arc<Self> {
        let mut names = Vec::with_capacity(fields.len());
        let mut json_names = Vec::with_capacity(fields.len());
        for (name, json_name) in fields {
            names.push(name.into_boxed_str());
            json_names.push(json_name.into_boxed_str());
        }
        let mut sorted_order: Vec<usize> = (0..names.len()).collect();
        sorted_order.sort_by(|&a, &b| names[a].cmp(&names[b]));
        let index = if names.len() > 8 {
            names
                .iter()
                .enumerate()
                .map(|(idx, name)| (name.clone(), idx))
                .collect()
        } else {
            HashMap::new()
        };
        Arc::new(Self {
            type_id,
            names: names.into_boxed_slice(),
            json_names: json_names.into_boxed_slice(),
            json_prefixes: OnceLock::new(),
            sorted_order: sorted_order.into_boxed_slice(),
            index,
        })
    }

    pub fn type_id(&self) -> &TypeId {
        &self.type_id
    }

    pub fn field_names(&self) -> &[Box<str>] {
        &self.names
    }

    pub fn json_names(&self) -> &[Box<str>] {
        &self.json_names
    }

    pub fn field_count(&self) -> usize {
        self.names.len()
    }

    pub fn sorted_order(&self) -> &[usize] {
        &self.sorted_order
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        if self.names.len() <= 8 {
            self.names.iter().position(|field| &**field == name)
        } else {
            self.index.get(name).copied()
        }
    }

    pub fn json_prefixes(&self) -> &[String] {
        self.json_prefixes.get_or_init(|| {
            self.json_names
                .iter()
                .map(|json_name| {
                    let mut prefix = String::with_capacity(json_name.len().saturating_add(3));
                    crate::native::json::write_json_escaped_string(json_name, &mut prefix);
                    prefix.push(':');
                    prefix
                })
                .collect::<Vec<_>>()
                .into_boxed_slice()
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MapKey {
    String(String),
    Int(i64),
    Bool(bool),
}

impl MapKey {
    pub fn to_value(&self) -> Value {
        match self {
            MapKey::String(s) => Value::String(s.clone()),
            MapKey::Int(n) => Value::Int(*n),
            MapKey::Bool(b) => Value::Bool(*b),
        }
    }

    pub fn from_value(val: &Value) -> Option<Self> {
        match val {
            Value::String(s) => Some(MapKey::String(s.clone())),
            Value::Int(n) => Some(MapKey::Int(*n)),
            Value::Bool(b) => Some(MapKey::Bool(*b)),
            _ => None,
        }
    }

    pub fn format_path_segment(&self) -> String {
        match self {
            MapKey::String(s) => format!("[{:?}]", s),
            MapKey::Int(n) => format!("[{}]", n),
            MapKey::Bool(b) => format!("[{}]", b),
        }
    }
}

impl std::fmt::Display for MapKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MapKey::String(s) => write!(f, "{}", s),
            MapKey::Int(n) => write!(f, "{}", n),
            MapKey::Bool(b) => write!(f, "{}", b),
        }
    }
}

#[derive(Debug, Clone)]
pub struct OrderedMap {
    entries: Vec<(MapKey, Value)>,
    index: HashMap<MapKey, usize>,
}

impl OrderedMap {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            index: HashMap::new(),
        }
    }

    pub fn set(&mut self, key: MapKey, val: Value) {
        if let Some(&pos) = self.index.get(&key) {
            self.entries[pos].1 = val;
        } else {
            let pos = self.entries.len();
            self.index.insert(key.clone(), pos);
            self.entries.push((key, val));
        }
    }

    pub fn get(&self, key: &MapKey) -> Option<&Value> {
        self.index.get(key).map(|&pos| &self.entries[pos].1)
    }

    pub fn remove(&mut self, key: &MapKey) -> Option<Value> {
        let pos = self.index.remove(key)?;
        let (_, removed_val) = self.entries.remove(pos);
        for (i, (k, _)) in self.entries.iter().enumerate().skip(pos) {
            self.index.insert(k.clone(), i);
        }
        Some(removed_val)
    }

    pub fn contains_key(&self, key: &MapKey) -> bool {
        self.index.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn entries(&self) -> &[(MapKey, Value)] {
        &self.entries
    }

    pub fn keys(&self) -> Vec<Value> {
        self.entries.iter().map(|(k, _)| k.to_value()).collect()
    }

    pub fn values(&self) -> Vec<Value> {
        self.entries.iter().map(|(_, v)| v.clone()).collect()
    }

    pub fn entry_tuples(&self) -> Vec<Value> {
        self.entries
            .iter()
            .map(|(k, v)| Value::Tuple(vec![k.to_value(), v.clone()]))
            .collect()
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

#[derive(Debug, Clone)]
pub struct ClosureData {
    pub params: Vec<Param>,
    pub body: Block,
    pub env: Arc<HashMap<String, Value>>,
    pub module: Option<PathBuf>,
}

#[derive(Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    String(String),
    Bool(bool),
    Nil,
    Void,
    List(Vec<Value>),
    Map(OrderedMap),
    Tuple(Vec<Value>),
    Struct {
        schema: Arc<StructSchema>,
        values: Vec<Value>,
    },
    Error {
        schema: Arc<StructSchema>,
        values: Vec<Value>,
    },
    Native(Arc<dyn NativeValue>),
    Function(String),
    Closure(Arc<ClosureData>),
    NativeFunction {
        module: String,
        name: String,
    },
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Int(n) => write!(f, "Int({})", n),
            Value::Float(v) => write!(f, "Float({})", v),
            Value::String(s) => write!(f, "String({:?})", s),
            Value::Bool(b) => write!(f, "Bool({})", b),
            Value::Nil => write!(f, "Nil"),
            Value::Void => write!(f, "Void"),
            Value::List(items) => write!(f, "List({:?})", items),
            Value::Map(map) => write!(f, "Map({:?})", map.entries()),
            Value::Tuple(items) => write!(f, "Tuple({:?})", items),
            Value::Struct { schema, values } => {
                write!(f, "Struct({}, {:?})", schema.type_id, values)
            }
            Value::Error { schema, values } => {
                write!(f, "Error({}, {:?})", schema.type_id, values)
            }
            Value::Native(nat) => write!(f, "Native({:?})", nat),
            Value::Function(name) => write!(f, "Function({})", name),
            Value::Closure(c) => write!(f, "Closure({} params)", c.params.len()),
            Value::NativeFunction { module, name } => write!(f, "NativeFunction({}.{})", module, name),
        }
    }
}

fn struct_fields_eq(
    s1: &Arc<StructSchema>,
    v1: &[Value],
    s2: &Arc<StructSchema>,
    v2: &[Value],
) -> bool {
    if s1.type_id != s2.type_id || v1.len() != v2.len() {
        return false;
    }
    if Arc::ptr_eq(s1, s2) {
        return v1 == v2;
    }
    s1.field_names().iter().enumerate().all(|(idx, name)| {
        s2.index_of(name)
            .is_some_and(|other_idx| v1[idx] == v2[other_idx])
    })
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Nil, Value::Nil) => true,
            (Value::Void, Value::Void) => true,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Map(a), Value::Map(b)) => a == b,
            (Value::Tuple(a), Value::Tuple(b)) => a == b,
            (
                Value::Struct { schema: s1, values: v1 },
                Value::Struct { schema: s2, values: v2 },
            ) => struct_fields_eq(s1, v1, s2, v2),
            (
                Value::Error { schema: s1, values: v1 },
                Value::Error { schema: s2, values: v2 },
            ) => struct_fields_eq(s1, v1, s2, v2),
            (Value::Native(a), Value::Native(b)) => a.dyn_eq(&**b),
            (Value::Function(a), Value::Function(b)) => a == b,
            (
                Value::NativeFunction { module: m1, name: n1 },
                Value::NativeFunction { module: m2, name: n2 },
            ) => m1 == m2 && n1 == n2,
            _ => false,
        }
    }
}

fn write_delimited(
    f: &mut std::fmt::Formatter<'_>,
    items: &[Value],
    open: &str,
    close: &str,
) -> std::fmt::Result {
    write!(f, "{}", open)?;
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            write!(f, ", ")?;
        }
        write!(f, "{}", item)?;
    }
    write!(f, "{}", close)
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Int(n) => write!(f, "{}", n),
            Value::Float(v) => write!(f, "{}", v),
            Value::String(s) => write!(f, "{}", s),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Nil => write!(f, "nil"),
            Value::Void => write!(f, "()"),
            Value::List(items) => write_delimited(f, items, "[", "]"),
            Value::Map(map) => {
                write!(f, "{{")?;
                for (i, (k, v)) in map.entries().iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    match k {
                        MapKey::String(s) => write!(f, "{:?}: {}", s, v)?,
                        MapKey::Int(n) => write!(f, "{}: {}", n, v)?,
                        MapKey::Bool(b) => write!(f, "{}: {}", b, v)?,
                    }
                }
                write!(f, "}}")
            }
            Value::Tuple(items) => write_delimited(f, items, "(", ")"),
            Value::Struct { schema, values } | Value::Error { schema, values } => {
                write!(f, "{}(", schema.type_id)?;
                let names = schema.field_names();
                for (pos, &idx) in schema.sorted_order().iter().enumerate() {
                    if pos > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", names[idx], values[idx])?;
                }
                write!(f, ")")
            }
            Value::Native(nat) => nat.display(f),
            Value::Function(name) => write!(f, "<func {}>", name),
            Value::Closure(_) => write!(f, "<closure>"),
            Value::NativeFunction { module, name } => write!(f, "<native func {}.{}>", module, name),
        }
    }
}

impl Value {
    pub fn struct_type_id(&self) -> Option<&TypeId> {
        match self {
            Value::Struct { schema, .. } | Value::Error { schema, .. } => Some(&schema.type_id),
            _ => None,
        }
    }

    pub fn field(&self, name: &str) -> Option<&Value> {
        match self {
            Value::Struct { schema, values } | Value::Error { schema, values } => {
                schema.index_of(name).and_then(|idx| values.get(idx))
            }
            _ => None,
        }
    }

    pub fn field_mut(&mut self, name: &str) -> Option<&mut Value> {
        match self {
            Value::Struct { schema, values } | Value::Error { schema, values } => {
                schema.index_of(name).and_then(|idx| values.get_mut(idx))
            }
            _ => None,
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Nil | Value::Void => false,
            Value::Int(n) => *n != 0,
            Value::Float(v) => *v != 0.0,
            Value::String(s) => !s.is_empty(),
            Value::List(l) => !l.is_empty(),
            Value::Map(m) => m.len() > 0,
            Value::Tuple(_) => true,
            Value::Struct { .. } | Value::Error { .. } => true,
            Value::Native(_) => true,
            Value::Function(_) | Value::Closure(_) | Value::NativeFunction { .. } => true,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "Int",
            Value::Float(_) => "Float",
            Value::String(_) => "String",
            Value::Bool(_) => "Bool",
            Value::Nil => "Nil",
            Value::Void => "Void",
            Value::List(_) => "List",
            Value::Map(_) => "Map",
            Value::Tuple(_) => "Tuple",
            Value::Struct { .. } => "Struct",
            Value::Error { .. } => "Error",
            Value::Native(nat) => nat.type_name(),
            Value::Function(_) | Value::Closure(_) | Value::NativeFunction { .. } => "Function",
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Value::Error { .. })
    }
}

pub fn builtin_error_schema(name: &str) -> Option<Arc<StructSchema>> {
    match name {
        "IndexError" => {
            static SCHEMA: OnceLock<Arc<StructSchema>> = OnceLock::new();
            Some(Arc::clone(SCHEMA.get_or_init(|| {
                StructSchema::new(
                    TypeId::builtin("IndexError"),
                    ["index".into(), "length".into(), "message".into()],
                )
            })))
        }
        "KeyError" => {
            static SCHEMA: OnceLock<Arc<StructSchema>> = OnceLock::new();
            Some(Arc::clone(SCHEMA.get_or_init(|| {
                StructSchema::new(
                    TypeId::builtin("KeyError"),
                    ["key".into(), "message".into()],
                )
            })))
        }
        "ParseError" => {
            static SCHEMA: OnceLock<Arc<StructSchema>> = OnceLock::new();
            Some(Arc::clone(SCHEMA.get_or_init(|| {
                StructSchema::new(
                    TypeId::builtin("ParseError"),
                    ["value".into(), "message".into()],
                )
            })))
        }
        _ => None,
    }
}

pub fn make_index_error(index: i64, length: usize) -> Value {
    let schema = builtin_error_schema("IndexError").unwrap();
    Value::Error {
        schema,
        values: vec![
            Value::Int(index),
            Value::Int(length as i64),
            Value::String(format!(
                "Index {} out of bounds for list of length {}",
                index, length
            )),
        ],
    }
}

pub fn make_key_error(key: &MapKey) -> Value {
    let key_str = key.to_string();
    let msg = format!("Key '{}' not found in map", key_str);
    let schema = builtin_error_schema("KeyError").unwrap();
    Value::Error {
        schema,
        values: vec![Value::String(key_str), Value::String(msg)],
    }
}

pub fn make_parse_error(value: &str, message: impl Into<String>) -> Value {
    let schema = builtin_error_schema("ParseError").unwrap();
    Value::Error {
        schema,
        values: vec![Value::String(value.to_string()), Value::String(message.into())],
    }
}
