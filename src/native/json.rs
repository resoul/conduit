use std::collections::HashMap;
use std::sync::Arc;

mod decode;
mod encode;
mod parser;

pub(crate) use decode::JsonDecodePlan;
pub(crate) use encode::{JsonEncodeSchema, encode_with_plan};

use super::{NativeFuncDef, NativeMethodDef, NativeModule, NativeTypeDef, NativeValue};
use crate::types::{Type, TypeId};
use crate::value::{MapKey, OrderedMap, Value};

const MAX_JSON_DEPTH: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub struct JsonNumber {
    pub raw: String,
    pub int_val: Option<i64>,
    pub float_val: f64,
    pub is_int_syntax: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JsonNode {
    Null,
    Bool(bool),
    Number(JsonNumber),
    String(String),
    Array(Vec<JsonNode>),
    Object(Vec<(String, JsonNode)>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct JsonValue(pub JsonNode);

impl NativeValue for JsonValue {
    fn type_id(&self) -> TypeId {
        TypeId::native("cdt.json", "Value")
    }

    fn type_name(&self) -> &'static str {
        "json.Value"
    }

    fn display(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", stringify_json_node(&self.0))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn dyn_eq(&self, other: &dyn NativeValue) -> bool {
        other
            .as_any()
            .downcast_ref::<JsonValue>()
            .map_or(false, |o| self.0 == o.0)
    }
}

pub fn make_decode_error(path: impl Into<String>, message: impl Into<String>) -> Value {
    let mut fields = HashMap::new();
    fields.insert("path".to_string(), Value::String(path.into()));
    fields.insert("message".to_string(), Value::String(message.into()));
    Value::Error {
        type_id: TypeId::native("cdt.json", "DecodeError"),
        fields,
    }
}

pub fn make_encode_error(path: impl Into<String>, message: impl Into<String>) -> Value {
    let mut fields = HashMap::new();
    fields.insert("path".to_string(), Value::String(path.into()));
    fields.insert("message".to_string(), Value::String(message.into()));
    Value::Error {
        type_id: TypeId::native("cdt.json", "EncodeError"),
        fields,
    }
}

pub fn join_field_path(base: &str, field: &str) -> String {
    // Bare segments follow the language's identifier syntax. Everything else
    // uses the same quoted bracket syntax as string map keys.
    let mut chars = field.chars();
    let is_identifier = chars
        .next()
        .is_some_and(|ch| ch.is_alphabetic() || ch == '_')
        && chars.all(|ch| ch.is_alphanumeric() || ch == '_');
    if !is_identifier {
        return format!(
            "{}{}",
            base,
            MapKey::String(field.to_owned()).format_path_segment()
        );
    }
    if base.is_empty() {
        field.to_string()
    } else {
        format!("{}.{}", base, field)
    }
}

pub fn join_index_path(base: &str, idx: usize) -> String {
    format!("{}[{}]", base, idx)
}

pub fn join_key_path(base: &str, key: &MapKey) -> String {
    format!("{}{}", base, key.format_path_segment())
}

fn prepend_error_path(mut prefix: String, child: &str) -> String {
    if !child.is_empty() {
        if !prefix.is_empty() && !child.starts_with('[') {
            prefix.push('.');
        }
        prefix.push_str(child);
    }
    prefix
}

pub fn parse_json(raw: &str) -> Result<JsonNode, (String, String)> {
    parser::parse_borrowed_json(raw).map(parser::BorrowedJsonNode::into_owned)
}

pub(crate) fn decode_json_with_plan(
    raw: &str,
    plan: &JsonDecodePlan,
    resolve_struct: &dyn Fn(&TypeId, &[Type]) -> Option<Vec<(String, String, Type)>>,
) -> Result<Value, (String, String)> {
    let node = parser::parse_borrowed_json(raw)?;
    decode::decode_borrowed_json(node, plan, "", resolve_struct)
}

pub fn write_json_escaped_string(s: &str, out: &mut String) {
    encode::write_escaped_string(s, out);
}

pub fn stringify_json_node(node: &JsonNode) -> String {
    let mut out = String::new();
    write_json_node(node, &mut out);
    out
}

fn write_json_node(node: &JsonNode, out: &mut String) {
    match node {
        JsonNode::Null => out.push_str("null"),
        JsonNode::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        JsonNode::Number(n) => {
            out.push_str(&n.raw);
            if !n.is_int_syntax
                && !n.raw.contains('.')
                && !n.raw.contains('e')
                && !n.raw.contains('E')
            {
                out.push_str(".0");
            }
        }
        JsonNode::String(s) => write_json_escaped_string(s, out),
        JsonNode::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json_node(item, out);
            }
            out.push(']');
        }
        JsonNode::Object(entries) => {
            out.push('{');
            for (i, (k, v)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_json_escaped_string(k, out);
                out.push(':');
                write_json_node(v, out);
            }
            out.push('}');
        }
    }
}

pub fn decode_node_to_conduit(
    node: &JsonNode,
    target_ty: &Type,
    path: &str,
    resolve_struct: &dyn Fn(&TypeId, &[Type]) -> Option<Vec<(String, Type)>>,
) -> Result<Value, (String, String)> {
    let resolve_struct_with_json_names =
        |type_id: &TypeId, type_args: &[Type]| -> Option<Vec<(String, String, Type)>> {
            resolve_struct(type_id, type_args).map(|fields| {
                fields
                    .into_iter()
                    .map(|(field_name, field_ty)| {
                        let json_name = field_name.clone();
                        (field_name, json_name, field_ty)
                    })
                    .collect()
            })
        };
    decode_node_to_conduit_with_json_names(node, target_ty, path, &resolve_struct_with_json_names)
}

#[derive(Debug)]
pub(crate) struct JsonDecodeSchema {
    fields: Vec<(String, String, Type)>,
    field_indexes: HashMap<String, usize>,
}

impl JsonDecodeSchema {
    pub(crate) fn new(fields: Vec<(String, String, Type)>) -> Self {
        let field_indexes = fields
            .iter()
            .enumerate()
            .map(|(index, (_, json_name, _))| (json_name.clone(), index))
            .collect();
        Self {
            fields,
            field_indexes,
        }
    }
}

pub(crate) fn decode_node_to_conduit_with_json_names(
    node: &JsonNode,
    target_ty: &Type,
    path: &str,
    resolve_struct: &dyn Fn(&TypeId, &[Type]) -> Option<Vec<(String, String, Type)>>,
) -> Result<Value, (String, String)> {
    let resolve_schema = |type_id: &TypeId, type_args: &[Type]| {
        resolve_struct(type_id, type_args).map(|fields| Arc::new(JsonDecodeSchema::new(fields)))
    };
    decode_node_to_conduit_with_schema(node, target_ty, path, &resolve_schema)
}

pub(crate) fn decode_node_to_conduit_with_schema(
    node: &JsonNode,
    target_ty: &Type,
    path: &str,
    resolve_struct: &dyn Fn(&TypeId, &[Type]) -> Option<Arc<JsonDecodeSchema>>,
) -> Result<Value, (String, String)> {
    match target_ty {
        Type::Optional(inner) => {
            if matches!(node, JsonNode::Null) {
                Ok(Value::Nil)
            } else {
                decode_node_to_conduit_with_schema(node, inner, path, resolve_struct)
            }
        }
        Type::Custom(tid) if tid == &TypeId::native("cdt.json", "Value") => {
            Ok(Value::Native(Arc::new(JsonValue(node.clone()))))
        }
        Type::Int => match node {
            JsonNode::Number(num) => {
                if !num.is_int_syntax {
                    Err((
                        path.to_string(),
                        format!(
                            "Expected Int, got fractional or exponential number '{}'",
                            num.raw
                        ),
                    ))
                } else if let Some(n) = num.int_val {
                    Ok(Value::Int(n))
                } else {
                    Err((
                        path.to_string(),
                        format!("Integer value '{}' overflows 64-bit Int", num.raw),
                    ))
                }
            }
            other => Err((
                path.to_string(),
                format!("Expected Int, got {}", json_kind_name(other)),
            )),
        },
        Type::Float => match node {
            JsonNode::Number(num) => Ok(Value::Float(num.float_val)),
            other => Err((
                path.to_string(),
                format!("Expected Float, got {}", json_kind_name(other)),
            )),
        },
        Type::Bool => match node {
            JsonNode::Bool(b) => Ok(Value::Bool(*b)),
            other => Err((
                path.to_string(),
                format!("Expected Bool, got {}", json_kind_name(other)),
            )),
        },
        Type::String => match node {
            JsonNode::String(s) => Ok(Value::String(s.clone())),
            other => Err((
                path.to_string(),
                format!("Expected String, got {}", json_kind_name(other)),
            )),
        },
        Type::List(inner) => match node {
            JsonNode::Array(items) => {
                let mut vals = Vec::with_capacity(items.len());
                for (i, item) in items.iter().enumerate() {
                    vals.push(
                        decode_node_to_conduit_with_schema(item, inner, "", resolve_struct)
                            .map_err(|(child, message)| {
                                (
                                    prepend_error_path(join_index_path(path, i), &child),
                                    message,
                                )
                            })?,
                    );
                }
                Ok(Value::List(vals))
            }
            other => Err((
                path.to_string(),
                format!("Expected List, got {}", json_kind_name(other)),
            )),
        },
        Type::Map(key_ty, val_ty) => {
            if **key_ty != Type::String {
                return Err((
                    path.to_string(),
                    format!(
                        "JSON objects can only decode to Map<String, V>, got Map<{}, {}>",
                        key_ty, val_ty
                    ),
                ));
            }
            match node {
                JsonNode::Object(entries) => {
                    let mut map = OrderedMap::new();
                    for (k, v) in entries {
                        let map_key = MapKey::String(k.clone());
                        let decoded_v =
                            decode_node_to_conduit_with_schema(v, val_ty, "", resolve_struct)
                                .map_err(|(child, message)| {
                                    (
                                        prepend_error_path(join_key_path(path, &map_key), &child),
                                        message,
                                    )
                                })?;
                        map.set(map_key, decoded_v);
                    }
                    Ok(Value::Map(map))
                }
                other => Err((
                    path.to_string(),
                    format!(
                        "Expected JSON object for Map<String, {}>, got {}",
                        val_ty,
                        json_kind_name(other)
                    ),
                )),
            }
        }
        Type::Tuple(elem_types) => match node {
            JsonNode::Array(items) => {
                if items.len() != elem_types.len() {
                    return Err((
                        path.to_string(),
                        format!(
                            "Expected tuple JSON array of length {}, got length {}",
                            elem_types.len(),
                            items.len()
                        ),
                    ));
                }
                let mut vals = Vec::with_capacity(elem_types.len());
                for (i, (item, elem_ty)) in items.iter().zip(elem_types.iter()).enumerate() {
                    vals.push(
                        decode_node_to_conduit_with_schema(item, elem_ty, "", resolve_struct)
                            .map_err(|(child, message)| {
                                (
                                    prepend_error_path(join_index_path(path, i), &child),
                                    message,
                                )
                            })?,
                    );
                }
                Ok(Value::Tuple(vals))
            }
            other => Err((
                path.to_string(),
                format!("Expected tuple JSON array, got {}", json_kind_name(other)),
            )),
        },
        Type::Custom(type_id) => decode_struct_node(node, type_id, &[], path, resolve_struct),
        Type::Generic { type_id, type_args } => {
            decode_struct_node(node, type_id, type_args, path, resolve_struct)
        }
        Type::Union(variants) => decode_union_node(node, variants, path, resolve_struct),
        other => Err((
            path.to_string(),
            format!("Unsupported target type '{}' for json.decode", other),
        )),
    }
}

fn decode_struct_node(
    node: &JsonNode,
    type_id: &TypeId,
    type_args: &[Type],
    path: &str,
    resolve_struct: &dyn Fn(&TypeId, &[Type]) -> Option<Arc<JsonDecodeSchema>>,
) -> Result<Value, (String, String)> {
    let declared_fields = resolve_struct(type_id, type_args).ok_or_else(|| {
        (
            path.to_string(),
            format!("Type '{}' cannot be decoded from JSON", type_id),
        )
    })?;
    let entries = match node {
        JsonNode::Object(e) => e,
        other => {
            return Err((
                path.to_string(),
                format!(
                    "Expected JSON object for '{}', got {}",
                    type_id,
                    json_kind_name(other)
                ),
            ));
        }
    };

    // Validate all keys before decoding fields, preserving error precedence and
    // the last occurrence of duplicate input keys without a per-object hash map.
    let mut json_fields = vec![None; declared_fields.fields.len()];
    for (k, v) in entries {
        let Some(&field_index) = declared_fields.field_indexes.get(k.as_str()) else {
            return Err((
                join_field_path(path, k),
                format!("Unknown field '{}' for type '{}'", k, type_id),
            ));
        };
        json_fields[field_index] = Some(v);
    }

    let mut out_fields = HashMap::with_capacity(declared_fields.fields.len());
    for (field_index, (field_name, json_name, field_ty)) in
        declared_fields.fields.iter().enumerate()
    {
        // Parser-defined schemas have unique names. The fallback also keeps
        // the legacy resolver API's behavior for repeated schema JSON names.
        let jval = json_fields[field_index].or_else(|| {
            declared_fields
                .field_indexes
                .get(json_name.as_str())
                .and_then(|&index| json_fields[index])
        });
        if let Some(jval) = jval {
            let decoded = decode_node_to_conduit_with_schema(jval, field_ty, "", resolve_struct)
                .map_err(|(child, message)| {
                    (
                        prepend_error_path(join_field_path(path, json_name), &child),
                        message,
                    )
                })?;
            out_fields.insert(field_name.clone(), decoded);
        } else if matches!(field_ty, Type::Optional(_)) || union_contains_nil(field_ty) {
            out_fields.insert(field_name.clone(), Value::Nil);
        } else {
            return Err((
                join_field_path(path, json_name),
                format!("Missing required field '{}'", json_name),
            ));
        }
    }

    Ok(Value::Struct {
        type_id: type_id.clone(),
        fields: out_fields,
    })
}

fn union_contains_nil(ty: &Type) -> bool {
    match ty {
        Type::Nil | Type::Optional(_) => true,
        Type::Union(vs) => vs.iter().any(union_contains_nil),
        _ => false,
    }
}

fn decode_union_node(
    node: &JsonNode,
    variants: &[Type],
    path: &str,
    resolve_struct: &dyn Fn(&TypeId, &[Type]) -> Option<Arc<JsonDecodeSchema>>,
) -> Result<Value, (String, String)> {
    let candidates: Vec<&Type> = match node {
        JsonNode::Null => variants
            .iter()
            .filter(|v| matches!(v, Type::Nil | Type::Optional(_)))
            .collect(),
        JsonNode::Bool(_) => variants
            .iter()
            .filter(|v| matches!(v, Type::Bool) || matches!(v, Type::Optional(inner) if **inner == Type::Bool))
            .collect(),
        JsonNode::String(_) => variants
            .iter()
            .filter(|v| matches!(v, Type::String) || matches!(v, Type::Optional(inner) if **inner == Type::String))
            .collect(),
        JsonNode::Number(number) => variants
            .iter()
            .filter(|v| {
                (number.is_int_syntax && matches!(v, Type::Int))
                    || (!number.is_int_syntax && matches!(v, Type::Float))
                    || matches!(v, Type::Optional(inner) if (number.is_int_syntax && **inner == Type::Int) || (!number.is_int_syntax && **inner == Type::Float))
            })
            .collect(),
        JsonNode::Array(_) => variants
            .iter()
            .filter(|v| matches!(v, Type::List(_) | Type::Tuple(_)))
            .collect(),
        JsonNode::Object(_) => variants
            .iter()
            .filter(|v| matches!(v, Type::Map(_, _) | Type::Custom(_) | Type::Generic { .. }))
            .collect(),
    };

    if candidates.len() > 1 {
        return Err((
            path.to_string(),
            format!("Ambiguous union target for JSON {}", json_kind_name(node)),
        ));
    }

    if let Some(target) = candidates.into_iter().next() {
        if matches!(node, JsonNode::Null) {
            return Ok(Value::Nil);
        }
        return decode_node_to_conduit_with_schema(node, target, path, resolve_struct);
    }

    if let Some(jv) = variants
        .iter()
        .find(|v| matches!(v, Type::Custom(id) if id == &TypeId::native("cdt.json", "Value")))
    {
        return decode_node_to_conduit_with_schema(node, jv, path, resolve_struct);
    }

    Err((
        path.to_string(),
        format!(
            "JSON {} does not match any variant of target union",
            json_kind_name(node)
        ),
    ))
}

fn json_kind_name(node: &JsonNode) -> &'static str {
    match node {
        JsonNode::Null => "null",
        JsonNode::Bool(_) => "bool",
        JsonNode::Number(_) => "number",
        JsonNode::String(_) => "string",
        JsonNode::Array(_) => "array",
        JsonNode::Object(_) => "object",
    }
}

pub fn encode_conduit_value(
    val: &Value,
    path: &str,
    out: &mut String,
    struct_field_order: &dyn Fn(&TypeId) -> Option<Vec<String>>,
) -> Result<(), (String, String)> {
    let struct_fields_with_json_names = |type_id: &TypeId| -> Option<Vec<(String, String)>> {
        struct_field_order(type_id).map(|field_names| {
            field_names
                .into_iter()
                .map(|field_name| {
                    let json_name = field_name.clone();
                    (field_name, json_name)
                })
                .collect()
        })
    };
    encode_conduit_value_with_json_names(val, path, out, &struct_fields_with_json_names)
}

pub(crate) fn encode_conduit_value_with_json_names(
    val: &Value,
    path: &str,
    out: &mut String,
    struct_fields: &dyn Fn(&TypeId) -> Option<Vec<(String, String)>>,
) -> Result<(), (String, String)> {
    let resolve_schema = |type_id: &TypeId| struct_fields(type_id).map(Arc::from);
    encode_conduit_value_with_schema(val, path, out, &resolve_schema)
}

pub(crate) fn encode_conduit_value_with_schema(
    val: &Value,
    path: &str,
    out: &mut String,
    struct_fields: &dyn Fn(&TypeId) -> Option<Arc<[(String, String)]>>,
) -> Result<(), (String, String)> {
    let resolve_schema = |type_id: &TypeId| {
        struct_fields(type_id)
            .map(|fields| Arc::new(JsonEncodeSchema::new(fields.as_ref().to_vec())))
    };
    encode_with_plan(val, path, out, &resolve_schema)
}

fn wrap_json_value(node: JsonNode) -> Value {
    Value::Native(Arc::new(JsonValue(node)))
}

fn with_json_value<F>(receiver: &Arc<dyn NativeValue>, f: F) -> Result<Value, Value>
where
    F: FnOnce(&JsonNode) -> Value,
{
    if let Some(jv) = receiver.as_any().downcast_ref::<JsonValue>() {
        Ok(f(&jv.0))
    } else {
        Ok(Value::Nil)
    }
}

pub fn create_json_module() -> NativeModule {
    let mut module = NativeModule::new(
        "cdt.json",
        "json",
        "Standard library for typed JSON decoding, encoding, and dynamic JSON trees",
    );

    let json_val_ty = Type::Custom(TypeId::native("cdt.json", "Value"));
    let decode_err_ty = Type::Custom(TypeId::native("cdt.json", "DecodeError"));
    let encode_err_ty = Type::Custom(TypeId::native("cdt.json", "EncodeError"));

    module.add_type(NativeTypeDef {
        name: "Value",
        qualified_name: "json.Value",
        doc: "Dynamic JSON node representing null, bool, number, string, array, or object",
        is_error: false,
        fields: vec![],
    });

    module.add_type(NativeTypeDef {
        name: "DecodeError",
        qualified_name: "json.DecodeError",
        doc: "Error returned when parsing or typed decoding of JSON fails",
        is_error: true,
        fields: vec![("path", Type::String), ("message", Type::String)],
    });

    module.add_type(NativeTypeDef {
        name: "EncodeError",
        qualified_name: "json.EncodeError",
        doc: "Error returned when encoding a value to JSON fails",
        is_error: true,
        fields: vec![("path", Type::String), ("message", Type::String)],
    });

    module.add_function(NativeFuncDef {
        name: "parse",
        doc: "Parses a JSON string into a dynamic json.Value tree",
        params: vec![("raw", Type::String)],
        return_type: Type::union(vec![json_val_ty.clone(), decode_err_ty.clone()]),
        is_validator: false,
        implementation: |args| {
            let raw = match args.into_iter().next() {
                Some(Value::String(s)) => s,
                _ => {
                    return Ok(make_decode_error(
                        "",
                        "Expected String argument for json.parse",
                    ))
                }
            };
            match parse_json(&raw) {
                Ok(node) => Ok(wrap_json_value(node)),
                Err((path, msg)) => Ok(make_decode_error(path, msg)),
            }
        },
    });

    module.add_function(NativeFuncDef {
        name: "stringify",
        doc: "Serializes a dynamic json.Value into a compact JSON string",
        params: vec![("val", json_val_ty.clone())],
        return_type: Type::String,
        is_validator: false,
        implementation: |args| {
            if let Some(Value::Native(nat)) = args.into_iter().next() {
                if let Some(jv) = nat.as_any().downcast_ref::<JsonValue>() {
                    return Ok(Value::String(stringify_json_node(&jv.0)));
                }
            }
            Ok(Value::String("null".to_string()))
        },
    });

    module.add_function(NativeFuncDef {
        name: "decode",
        doc: "Decodes a JSON string into target schema type T",
        params: vec![("raw", Type::String)],
        return_type: Type::union(vec![Type::Any, decode_err_ty]),
        is_validator: false,
        implementation: |_args| {
            Ok(make_decode_error(
                "",
                "json.decode requires a target type argument",
            ))
        },
    });

    module.add_function(NativeFuncDef {
        name: "encode",
        doc: "Serializes a Conduit value into a deterministic JSON string",
        params: vec![("value", Type::Any)],
        return_type: Type::union(vec![Type::String, encode_err_ty]),
        is_validator: false,
        implementation: |args| {
            let val = args.into_iter().next().unwrap_or(Value::Nil);
            let mut out = String::new();
            match encode_conduit_value(&val, "", &mut out, &|_| None) {
                Ok(()) => Ok(Value::String(out)),
                Err((path, msg)) => Ok(make_encode_error(path, msg)),
            }
        },
    });

    module.add_method(
        "Value",
        NativeMethodDef {
            name: "is_null",
            doc: "Returns true if this JSON value is null",
            params: vec![],
            return_type: Type::Bool,
            implementation: |recv, _| {
                with_json_value(recv, |node| Value::Bool(matches!(node, JsonNode::Null)))
            },
        },
    );

    module.add_method(
        "Value",
        NativeMethodDef {
            name: "as_string",
            doc: "Extracts a String if this node is a JSON string, or nil",
            params: vec![],
            return_type: Type::Optional(Box::new(Type::String)),
            implementation: |recv, _| {
                with_json_value(recv, |node| match node {
                    JsonNode::String(s) => Value::String(s.clone()),
                    _ => Value::Nil,
                })
            },
        },
    );

    module.add_method(
        "Value",
        NativeMethodDef {
            name: "as_int",
            doc: "Extracts an Int if this node is an exact JSON integer fitting i64, or nil",
            params: vec![],
            return_type: Type::Optional(Box::new(Type::Int)),
            implementation: |recv, _| {
                with_json_value(recv, |node| match node {
                    JsonNode::Number(n) if n.is_int_syntax => {
                        n.int_val.map(Value::Int).unwrap_or(Value::Nil)
                    }
                    _ => Value::Nil,
                })
            },
        },
    );

    module.add_method(
        "Value",
        NativeMethodDef {
            name: "as_float",
            doc: "Extracts a Float if this node is a JSON number, or nil",
            params: vec![],
            return_type: Type::Optional(Box::new(Type::Float)),
            implementation: |recv, _| {
                with_json_value(recv, |node| match node {
                    JsonNode::Number(n) => Value::Float(n.float_val),
                    _ => Value::Nil,
                })
            },
        },
    );

    module.add_method(
        "Value",
        NativeMethodDef {
            name: "as_bool",
            doc: "Extracts a Bool if this node is a JSON boolean, or nil",
            params: vec![],
            return_type: Type::Optional(Box::new(Type::Bool)),
            implementation: |recv, _| {
                with_json_value(recv, |node| match node {
                    JsonNode::Bool(b) => Value::Bool(*b),
                    _ => Value::Nil,
                })
            },
        },
    );

    module.add_method(
        "Value",
        NativeMethodDef {
            name: "as_list",
            doc: "Extracts a list of json.Value elements if this node is a JSON array, or nil",
            params: vec![],
            return_type: Type::Optional(Box::new(Type::List(Box::new(json_val_ty.clone())))),
            implementation: |recv, _| {
                with_json_value(recv, |node| match node {
                    JsonNode::Array(items) => {
                        Value::List(items.iter().cloned().map(wrap_json_value).collect())
                    }
                    _ => Value::Nil,
                })
            },
        },
    );

    module.add_method(
        "Value",
        NativeMethodDef {
            name: "as_map",
            doc:
                "Extracts an ordered Map<String, json.Value> if this node is a JSON object, or nil",
            params: vec![],
            return_type: Type::Optional(Box::new(Type::Map(
                Box::new(Type::String),
                Box::new(json_val_ty),
            ))),
            implementation: |recv, _| {
                with_json_value(recv, |node| match node {
                    JsonNode::Object(entries) => {
                        let mut map = OrderedMap::new();
                        for (k, v) in entries {
                            map.set(MapKey::String(k.clone()), wrap_json_value(v.clone()));
                        }
                        Value::Map(map)
                    }
                    _ => Value::Nil,
                })
            },
        },
    );

    module
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn json_paths_quote_non_identifier_keys() {
        assert_eq!(join_field_path("", "name"), "name");
        assert_eq!(join_field_path("items[0]", "имя_2"), "items[0].имя_2");
        assert_eq!(join_field_path("", "a.b"), "[\"a.b\"]");
        assert_eq!(join_field_path("items[0]", "x[0]"), "items[0][\"x[0]\"]");
        assert_eq!(join_field_path("", ""), "[\"\"]");
        assert_eq!(join_field_path("", "a\"b\n"), "[\"a\\\"b\\n\"]");
        assert_eq!(join_field_path("", "0name"), "[\"0name\"]");
    }

    #[test]
    fn syntax_decode_and_encode_errors_use_quoted_json_names() {
        let syntax_err = parse_json(r#"{"outer":{"a.b":!}}"#).unwrap_err();
        assert_eq!(syntax_err.0, "outer[\"a.b\"]");

        let type_id = TypeId::program("Tagged");
        let ty = Type::Custom(type_id.clone());
        let schema =
            |_: &TypeId, _: &[Type]| Some(vec![("x".to_owned(), "a.b".to_owned(), Type::Int)]);
        let node = parse_json(r#"{"a.b":"wrong"}"#).unwrap();
        let decode_err =
            decode_node_to_conduit_with_json_names(&node, &ty, "", &schema).unwrap_err();
        assert_eq!(decode_err.0, "[\"a.b\"]");

        let fields = HashMap::from([("x".to_owned(), Value::Function("f".to_owned()))]);
        let value = Value::Struct { type_id, fields };
        let names = |_: &TypeId| Some(vec![("x".to_owned(), "a.b".to_owned())]);
        let mut output = String::new();
        let encode_err =
            encode_conduit_value_with_json_names(&value, "", &mut output, &names).unwrap_err();
        assert_eq!(encode_err.0, "[\"a.b\"]");
    }

    #[test]
    fn indexed_schema_preserves_duplicate_keys_and_error_precedence() {
        let type_id = TypeId::program("Tagged");
        let ty = Type::Custom(type_id.clone());
        let schema = Arc::new(JsonDecodeSchema::new(vec![
            ("x".to_owned(), "display_name".to_owned(), Type::Int),
            (
                "note".to_owned(),
                "note".to_owned(),
                Type::Optional(Box::new(Type::String)),
            ),
        ]));
        let resolve = |_: &TypeId, _: &[Type]| Some(schema.clone());
        let node = parse_json(r#"{"display_name":"ignored","display_name":7}"#).unwrap();
        let value = decode_node_to_conduit_with_schema(&node, &ty, "", &resolve).unwrap();
        assert_eq!(
            value,
            Value::Struct {
                type_id,
                fields: HashMap::from([
                    ("x".to_owned(), Value::Int(7)),
                    ("note".to_owned(), Value::Nil),
                ]),
            }
        );

        let node = parse_json(r#"{"display_name":7,"display_name":false}"#).unwrap();
        let error = decode_node_to_conduit_with_schema(&node, &ty, "", &resolve).unwrap_err();
        assert_eq!(error.0, "display_name");
        assert!(error.1.contains("Expected Int"));

        for raw in [
            r#"{"display_name":false,"unexpected":1}"#,
            r#"{"unexpected":1}"#,
        ] {
            let node = parse_json(raw).unwrap();
            let error = decode_node_to_conduit_with_schema(&node, &ty, "", &resolve).unwrap_err();
            assert_eq!(error.0, "unexpected");
            assert!(error.1.contains("Unknown field"));
        }
    }

    #[test]
    fn legacy_resolver_keeps_repeated_schema_names_compatible() {
        let type_id = TypeId::program("Legacy");
        let ty = Type::Custom(type_id.clone());
        let node = parse_json(r#"{"key":7}"#).unwrap();
        let resolve = |_: &TypeId, _: &[Type]| {
            Some(vec![
                ("x".to_owned(), "key".to_owned(), Type::Int),
                ("y".to_owned(), "key".to_owned(), Type::Int),
            ])
        };
        let value = decode_node_to_conduit_with_json_names(&node, &ty, "", &resolve).unwrap();
        assert_eq!(
            value,
            Value::Struct {
                type_id,
                fields: HashMap::from([
                    ("x".to_owned(), Value::Int(7)),
                    ("y".to_owned(), Value::Int(7)),
                ]),
            }
        );
    }

    #[test]
    fn lazy_paths_preserve_nested_list_map_tuple_and_tag_segments() {
        let type_id = TypeId::program("Tagged");
        let ty = Type::List(Box::new(Type::Map(
            Box::new(Type::String),
            Box::new(Type::Tuple(vec![Type::Custom(type_id.clone())])),
        )));
        let schema = Arc::new(JsonDecodeSchema::new(vec![(
            "x".to_owned(),
            "a.b".to_owned(),
            Type::Int,
        )]));
        let resolve = |_: &TypeId, _: &[Type]| Some(schema.clone());
        let node = parse_json(r#"[{"metadata":[{"a.b":false}]}]"#).unwrap();
        let error =
            decode_node_to_conduit_with_schema(&node, &ty, "payload", &resolve).unwrap_err();
        assert_eq!(error.0, "payload[0][\"metadata\"][0][\"a.b\"]");

        let mut map = OrderedMap::new();
        map.set(
            MapKey::String("metadata".to_owned()),
            Value::Tuple(vec![Value::Struct {
                type_id,
                fields: HashMap::from([("x".to_owned(), Value::Function("f".to_owned()))]),
            }]),
        );
        let value = Value::List(vec![Value::Map(map)]);
        let fields: Arc<[(String, String)]> = vec![("x".to_owned(), "a.b".to_owned())].into();
        let resolve = |_: &TypeId| Some(fields.clone());
        let mut out = String::new();
        let error =
            encode_conduit_value_with_schema(&value, "payload", &mut out, &resolve).unwrap_err();
        assert_eq!(error.0, "payload[0][\"metadata\"][0][\"a.b\"]");
    }
}
