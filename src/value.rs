use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::ast::{Block, Param};
use crate::native::NativeValue;
use crate::types::TypeId;

#[derive(Debug, Clone)]
pub struct ClosureData {
    pub params: Vec<Param>,
    pub body: Block,
    pub env: HashMap<String, Value>,
    pub module: Option<PathBuf>,
}

#[derive(Clone)]
pub enum Value {
    Int(i64),
    String(String),
    Bool(bool),
    Nil,
    Void,
    List(Vec<Value>),
    Tuple(Vec<Value>),
    Struct {
        type_id: TypeId,
        fields: HashMap<String, Value>,
    },
    Error {
        type_id: TypeId,
        fields: HashMap<String, Value>,
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
            Value::String(s) => write!(f, "String({:?})", s),
            Value::Bool(b) => write!(f, "Bool({})", b),
            Value::Nil => write!(f, "Nil"),
            Value::Void => write!(f, "Void"),
            Value::List(items) => write!(f, "List({:?})", items),
            Value::Tuple(items) => write!(f, "Tuple({:?})", items),
            Value::Struct { type_id, fields } => write!(f, "Struct({}, {:?})", type_id, fields),
            Value::Error { type_id, fields } => write!(f, "Error({}, {:?})", type_id, fields),
            Value::Native(nat) => write!(f, "Native({:?})", nat),
            Value::Function(name) => write!(f, "Function({})", name),
            Value::Closure(c) => write!(f, "Closure({} params)", c.params.len()),
            Value::NativeFunction { module, name } => write!(f, "NativeFunction({}.{})", module, name),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Nil, Value::Nil) => true,
            (Value::Void, Value::Void) => true,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Tuple(a), Value::Tuple(b)) => a == b,
            (
                Value::Struct { type_id: t1, fields: f1 },
                Value::Struct { type_id: t2, fields: f2 },
            ) => t1 == t2 && f1 == f2,
            (
                Value::Error { type_id: t1, fields: f1 },
                Value::Error { type_id: t2, fields: f2 },
            ) => t1 == t2 && f1 == f2,
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
            Value::String(s) => write!(f, "{}", s),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Nil => write!(f, "nil"),
            Value::Void => write!(f, "()"),
            Value::List(items) => write_delimited(f, items, "[", "]"),
            Value::Tuple(items) => write_delimited(f, items, "(", ")"),
            Value::Struct { type_id, fields } | Value::Error { type_id, fields } => {
                write!(f, "{}(", type_id)?;
                let mut first = true;
                for (k, v) in fields {
                    if !first {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", k, v)?;
                    first = false;
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
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Nil | Value::Void => false,
            Value::Int(n) => *n != 0,
            Value::String(s) => !s.is_empty(),
            Value::List(l) => !l.is_empty(),
            Value::Tuple(_) => true,
            Value::Struct { .. } | Value::Error { .. } => true,
            Value::Native(_) => true,
            Value::Function(_) | Value::Closure(_) | Value::NativeFunction { .. } => true,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "Int",
            Value::String(_) => "String",
            Value::Bool(_) => "Bool",
            Value::Nil => "Nil",
            Value::Void => "Void",
            Value::List(_) => "List",
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

pub fn make_index_error(index: i64, length: usize) -> Value {
    let mut fields = HashMap::new();
    fields.insert("index".to_string(), Value::Int(index));
    fields.insert("length".to_string(), Value::Int(length as i64));
    fields.insert(
        "message".to_string(),
        Value::String(format!("Index {} out of bounds for list of length {}", index, length)),
    );
    Value::Error {
        type_id: TypeId::builtin("IndexError"),
        fields,
    }
}
