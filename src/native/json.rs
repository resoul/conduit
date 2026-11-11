use std::sync::{Arc, OnceLock};

mod decode;
mod encode;
mod parser;

pub(crate) use decode::JsonDecodePlan;
pub use encode::encode_with_struct_schemas;

use super::{NativeFuncDef, NativeMethodDef, NativeModule, NativeTypeDef, NativeValue};
use crate::types::{Type, TypeId};
use crate::value::{MapKey, OrderedMap, StructSchema, Value};

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
    static SCHEMA: OnceLock<Arc<StructSchema>> = OnceLock::new();
    let schema = Arc::clone(SCHEMA.get_or_init(|| {
        StructSchema::new(
            TypeId::native("cdt.json", "DecodeError"),
            ["path".into(), "message".into()],
        )
    }));
    Value::Error {
        schema,
        values: vec![Value::String(path.into()), Value::String(message.into())],
    }
}

pub fn make_encode_error(path: impl Into<String>, message: impl Into<String>) -> Value {
    static SCHEMA: OnceLock<Arc<StructSchema>> = OnceLock::new();
    let schema = Arc::clone(SCHEMA.get_or_init(|| {
        StructSchema::new(
            TypeId::native("cdt.json", "EncodeError"),
            ["path".into(), "message".into()],
        )
    }));
    Value::Error {
        schema,
        values: vec![Value::String(path.into()), Value::String(message.into())],
    }
}

pub fn join_field_path(base: &str, field: &str) -> String {
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

pub fn decode_json(raw: &str, target_ty: &Type) -> Result<Value, (String, String)> {
    let no_structs = |_: &TypeId, _: &[Type]| None;
    let plan = JsonDecodePlan::compile(target_ty, &no_structs);
    decode_json_with_plan(raw, &plan, &no_structs)
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
            match encode_with_struct_schemas(&val, "", &mut out) {
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
        assert_eq!(join_field_path("items[0]", "name_2"), "items[0].name_2");
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
        let plan = JsonDecodePlan::compile(&ty, &schema);
        let decode_err = decode_json_with_plan(r#"{"a.b":"wrong"}"#, &plan, &schema).unwrap_err();
        assert_eq!(decode_err.0, "[\"a.b\"]");

        let value = Value::Struct {
            schema: StructSchema::with_json_names(
                type_id,
                vec![("x".to_owned(), "a.b".to_owned())],
            ),
            values: vec![Value::Function("f".to_owned())],
        };
        let mut output = String::new();
        let encode_err = encode_with_struct_schemas(&value, "", &mut output).unwrap_err();
        assert_eq!(encode_err.0, "[\"a.b\"]");
    }

    #[test]
    fn indexed_schema_preserves_duplicate_keys_and_error_precedence() {
        let type_id = TypeId::program("Tagged");
        let ty = Type::Custom(type_id.clone());
        let resolve = |_: &TypeId, _: &[Type]| {
            Some(vec![
                ("x".to_owned(), "display_name".to_owned(), Type::Int),
                (
                    "note".to_owned(),
                    "note".to_owned(),
                    Type::Optional(Box::new(Type::String)),
                ),
            ])
        };
        let plan = JsonDecodePlan::compile(&ty, &resolve);
        let value = decode_json_with_plan(
            r#"{"display_name":"ignored","display_name":7}"#,
            &plan,
            &resolve,
        )
        .unwrap();
        assert_eq!(
            value,
            Value::Struct {
                schema: StructSchema::with_json_names(
                    type_id,
                    vec![
                        ("x".to_owned(), "display_name".to_owned()),
                        ("note".to_owned(), "note".to_owned()),
                    ],
                ),
                values: vec![Value::Int(7), Value::Nil],
            }
        );

        let error = decode_json_with_plan(
            r#"{"display_name":7,"display_name":false}"#,
            &plan,
            &resolve,
        )
        .unwrap_err();
        assert_eq!(error.0, "display_name");
        assert!(error.1.contains("Expected Int"));

        for raw in [
            r#"{"display_name":false,"unexpected":1}"#,
            r#"{"unexpected":1}"#,
        ] {
            let error = decode_json_with_plan(raw, &plan, &resolve).unwrap_err();
            assert_eq!(error.0, "unexpected");
            assert!(error.1.contains("Unknown field"));
        }
    }

    #[test]
    fn lazy_paths_preserve_nested_list_map_tuple_and_tag_segments() {
        let type_id = TypeId::program("Tagged");
        let ty = Type::List(Box::new(Type::Map(
            Box::new(Type::String),
            Box::new(Type::Tuple(vec![Type::Custom(type_id.clone())])),
        )));
        let resolve =
            |_: &TypeId, _: &[Type]| Some(vec![("x".to_owned(), "a.b".to_owned(), Type::Int)]);
        let plan = JsonDecodePlan::compile(&ty, &resolve);
        let node = parser::parse_borrowed_json(r#"[{"metadata":[{"a.b":false}]}]"#).unwrap();
        let error = decode::decode_borrowed_json(node, &plan, "payload", &resolve).unwrap_err();
        assert_eq!(error.0, "payload[0][\"metadata\"][0][\"a.b\"]");

        let mut map = OrderedMap::new();
        map.set(
            MapKey::String("metadata".to_owned()),
            Value::Tuple(vec![Value::Struct {
                schema: StructSchema::with_json_names(
                    type_id,
                    vec![("x".to_owned(), "a.b".to_owned())],
                ),
                values: vec![Value::Function("f".to_owned())],
            }]),
        );
        let value = Value::List(vec![Value::Map(map)]);
        let mut out = String::new();
        let error = encode_with_struct_schemas(&value, "payload", &mut out).unwrap_err();
        assert_eq!(error.0, "payload[0][\"metadata\"][0][\"a.b\"]");
    }
}
