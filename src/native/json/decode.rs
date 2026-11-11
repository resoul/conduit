use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::types::{Type, TypeId};
use crate::value::{MapKey, OrderedMap, StructSchema, Value};

use super::parser::BorrowedJsonNode;
use super::{join_field_path, join_index_path, join_key_path, prepend_error_path, JsonValue};

type ResolveStruct<'a> = dyn Fn(&TypeId, &[Type]) -> Option<Vec<(String, String, Type)>> + 'a;

#[derive(Debug)]
pub(crate) struct JsonDecodePlan {
    nodes: Vec<PlanNode>,
    root: usize,
}

#[derive(Debug)]
enum PlanNode {
    Pending,
    Int,
    Float,
    Bool,
    String,
    Dynamic,
    Optional(usize),
    List(usize),
    Map { value: usize, value_name: String },
    Tuple(Vec<usize>),
    Struct(StructPlan),
    Union(UnionPlan),
    Unavailable(TypeId),
    Unsupported(Type),
    InvalidMap(String),
    Deferred(Type),
}

#[derive(Debug)]
struct StructPlan {
    struct_schema: Arc<StructSchema>,
    fields: Vec<FieldPlan>,
    field_indexes: HashMap<String, usize>,
    has_duplicate_json_names: bool,
}

#[derive(Debug)]
struct FieldPlan {
    json_name: String,
    child: usize,
    is_optional: bool,
}

#[derive(Clone, Copy, Debug)]
enum UnionChoice {
    Missing,
    Ambiguous,
    Target(usize),
}

#[derive(Debug)]
struct UnionPlan {
    choices: [UnionChoice; 7],
    dynamic_fallback: Option<usize>,
}

impl JsonDecodePlan {
    pub(crate) fn compile(target: &Type, resolve: &ResolveStruct<'_>) -> Self {
        let mut compiler = Compiler {
            nodes: Vec::new(),
            indexes: BTreeMap::new(),
            active_structs: Vec::new(),
            resolve,
        };
        let root = compiler.add(target);
        Self {
            nodes: compiler.nodes,
            root,
        }
    }
}

struct Compiler<'a> {
    nodes: Vec<PlanNode>,
    indexes: BTreeMap<Type, usize>,
    active_structs: Vec<TypeId>,
    resolve: &'a ResolveStruct<'a>,
}

impl Compiler<'_> {
    fn add(&mut self, ty: &Type) -> usize {
        if let Some(&index) = self.indexes.get(ty) {
            return index;
        }
        let index = self.nodes.len();
        self.nodes.push(PlanNode::Pending);
        self.indexes.insert(ty.clone(), index);
        let node = match ty {
            Type::Int => PlanNode::Int,
            Type::Float => PlanNode::Float,
            Type::Bool => PlanNode::Bool,
            Type::String => PlanNode::String,
            Type::Optional(inner) => PlanNode::Optional(self.add(inner)),
            Type::List(inner) => PlanNode::List(self.add(inner)),
            Type::Map(key, value) => {
                if **key != Type::String {
                    PlanNode::InvalidMap(format!(
                        "JSON objects can only decode to Map<String, V>, got Map<{}, {}>",
                        key, value
                    ))
                } else {
                    PlanNode::Map {
                        value: self.add(value),
                        value_name: value.to_string(),
                    }
                }
            }
            Type::Tuple(elements) => {
                PlanNode::Tuple(elements.iter().map(|element| self.add(element)).collect())
            }
            Type::Custom(type_id) if type_id == &TypeId::native("cdt.json", "Value") => {
                PlanNode::Dynamic
            }
            Type::Custom(type_id) => self.add_struct(ty, type_id, &[]),
            Type::Generic { type_id, type_args } => self.add_struct(ty, type_id, type_args),
            Type::Union(variants) => {
                let children: Vec<_> = variants.iter().map(|variant| self.add(variant)).collect();
                let mut choices = [UnionChoice::Missing; 7];
                for (variant, &child) in variants.iter().zip(&children) {
                    for (kind, choice) in choices.iter_mut().enumerate() {
                        if union_candidate(variant, kind) {
                            *choice = match *choice {
                                UnionChoice::Missing => UnionChoice::Target(child),
                                _ => UnionChoice::Ambiguous,
                            };
                        }
                    }
                }
                let dynamic_fallback = variants.iter().zip(children).find_map(|(ty, child)| {
                    matches!(ty, Type::Custom(id) if id == &TypeId::native("cdt.json", "Value"))
                        .then_some(child)
                });
                PlanNode::Union(UnionPlan {
                    choices,
                    dynamic_fallback,
                })
            }
            other => PlanNode::Unsupported(other.clone()),
        };
        self.nodes[index] = node;
        index
    }

    fn add_struct(&mut self, ty: &Type, type_id: &TypeId, type_args: &[Type]) -> PlanNode {
        if self.active_structs.contains(type_id) {
            return PlanNode::Deferred(ty.clone());
        }
        let Some(definitions) = (self.resolve)(type_id, type_args) else {
            return PlanNode::Unavailable(type_id.clone());
        };
        self.active_structs.push(type_id.clone());
        let use_map = definitions.len() > 8;
        let mut fields = Vec::with_capacity(definitions.len());
        let mut schema_names = Vec::with_capacity(definitions.len());
        let mut field_indexes = if use_map {
            HashMap::with_capacity(definitions.len())
        } else {
            HashMap::new()
        };
        let mut seen_names = if !use_map {
            Vec::with_capacity(definitions.len())
        } else {
            Vec::new()
        };
        let mut has_duplicate_json_names = false;
        for (name, json_name, ty) in definitions {
            let index = fields.len();
            if use_map {
                has_duplicate_json_names |= field_indexes.insert(json_name.clone(), index).is_some();
            } else {
                has_duplicate_json_names |= seen_names.iter().any(|prev| prev == &json_name);
                seen_names.push(json_name.clone());
            }
            schema_names.push((name, json_name.clone()));
            fields.push(FieldPlan {
                json_name,
                is_optional: union_contains_nil(&ty),
                child: self.add(&ty),
            });
        }
        self.active_structs.pop();
        let struct_schema = StructSchema::with_json_names(type_id.clone(), schema_names);
        PlanNode::Struct(StructPlan {
            struct_schema,
            fields,
            field_indexes,
            has_duplicate_json_names,
        })
    }
}

fn union_contains_nil(ty: &Type) -> bool {
    match ty {
        Type::Nil | Type::Optional(_) => true,
        Type::Union(variants) => variants.iter().any(union_contains_nil),
        _ => false,
    }
}

fn union_candidate(ty: &Type, kind: usize) -> bool {
    match kind {
        0 => matches!(ty, Type::Nil | Type::Optional(_)),
        1 => {
            matches!(ty, Type::Bool) || matches!(ty, Type::Optional(inner) if **inner == Type::Bool)
        }
        2 => {
            matches!(ty, Type::String)
                || matches!(ty, Type::Optional(inner) if **inner == Type::String)
        }
        3 => matches!(ty, Type::Int) || matches!(ty, Type::Optional(inner) if **inner == Type::Int),
        4 => {
            matches!(ty, Type::Float)
                || matches!(ty, Type::Optional(inner) if **inner == Type::Float)
        }
        5 => matches!(ty, Type::List(_) | Type::Tuple(_)),
        6 => matches!(ty, Type::Map(_, _) | Type::Custom(_) | Type::Generic { .. }),
        _ => unreachable!(),
    }
}

pub(super) fn decode_borrowed_json(
    node: BorrowedJsonNode<'_>,
    plan: &JsonDecodePlan,
    path: &str,
    resolve: &ResolveStruct<'_>,
) -> Result<Value, (String, String)> {
    decode_node(node, plan, plan.root, path, resolve)
}

fn decode_node(
    node: BorrowedJsonNode<'_>,
    plan: &JsonDecodePlan,
    index: usize,
    path: &str,
    resolve: &ResolveStruct<'_>,
) -> Result<Value, (String, String)> {
    match &plan.nodes[index] {
        PlanNode::Pending => unreachable!("decode plan compilation left an unfinished node"),
        PlanNode::Optional(inner) => {
            if matches!(node, BorrowedJsonNode::Null) {
                Ok(Value::Nil)
            } else {
                decode_node(node, plan, *inner, path, resolve)
            }
        }
        PlanNode::Dynamic => Ok(Value::Native(Arc::new(JsonValue(node.into_owned())))),
        PlanNode::Int => match node {
            BorrowedJsonNode::Number(number) => {
                if !number.is_int_syntax {
                    Err((
                        path.to_owned(),
                        format!(
                            "Expected Int, got fractional or exponential number '{}'",
                            number.raw
                        ),
                    ))
                } else if let Some(value) = number.int_val {
                    Ok(Value::Int(value))
                } else {
                    Err((
                        path.to_owned(),
                        format!("Integer value '{}' overflows 64-bit Int", number.raw),
                    ))
                }
            }
            other => Err((
                path.to_owned(),
                format!("Expected Int, got {}", kind_name(&other)),
            )),
        },
        PlanNode::Float => match node {
            BorrowedJsonNode::Number(number) => Ok(Value::Float(number.float_val)),
            other => Err((
                path.to_owned(),
                format!("Expected Float, got {}", kind_name(&other)),
            )),
        },
        PlanNode::Bool => match node {
            BorrowedJsonNode::Bool(value) => Ok(Value::Bool(value)),
            other => Err((
                path.to_owned(),
                format!("Expected Bool, got {}", kind_name(&other)),
            )),
        },
        PlanNode::String => match node {
            BorrowedJsonNode::String(value) => Ok(Value::String(value.into_owned())),
            other => Err((
                path.to_owned(),
                format!("Expected String, got {}", kind_name(&other)),
            )),
        },
        PlanNode::List(inner) => match node {
            BorrowedJsonNode::Array(items) => {
                let mut values = Vec::with_capacity(items.len());
                for (index, item) in items.into_iter().enumerate() {
                    values.push(decode_node(item, plan, *inner, "", resolve).map_err(
                        |(child, message)| {
                            (
                                prepend_error_path(join_index_path(path, index), &child),
                                message,
                            )
                        },
                    )?);
                }
                Ok(Value::List(values))
            }
            other => Err((
                path.to_owned(),
                format!("Expected List, got {}", kind_name(&other)),
            )),
        },
        PlanNode::Map { value, value_name } => match node {
            BorrowedJsonNode::Object(entries) => {
                let mut values = OrderedMap::new();
                for (key, entry) in entries {
                    let key = MapKey::String(key.into_owned());
                    let decoded = decode_node(entry, plan, *value, "", resolve).map_err(
                        |(child, message)| {
                            (
                                prepend_error_path(join_key_path(path, &key), &child),
                                message,
                            )
                        },
                    )?;
                    values.set(key, decoded);
                }
                Ok(Value::Map(values))
            }
            other => Err((
                path.to_owned(),
                format!(
                    "Expected JSON object for Map<String, {}>, got {}",
                    value_name,
                    kind_name(&other)
                ),
            )),
        },
        PlanNode::Tuple(elements) => match node {
            BorrowedJsonNode::Array(items) => {
                if items.len() != elements.len() {
                    return Err((
                        path.to_owned(),
                        format!(
                            "Expected tuple JSON array of length {}, got length {}",
                            elements.len(),
                            items.len()
                        ),
                    ));
                }
                let mut values = Vec::with_capacity(items.len());
                for (index, (item, &child_plan)) in items.into_iter().zip(elements).enumerate() {
                    values.push(decode_node(item, plan, child_plan, "", resolve).map_err(
                        |(child, message)| {
                            (
                                prepend_error_path(join_index_path(path, index), &child),
                                message,
                            )
                        },
                    )?);
                }
                Ok(Value::Tuple(values))
            }
            other => Err((
                path.to_owned(),
                format!("Expected tuple JSON array, got {}", kind_name(&other)),
            )),
        },
        PlanNode::Struct(schema) => decode_struct(node, schema, plan, path, resolve),
        PlanNode::Union(union) => {
            let kind = union_kind(&node);
            match union.choices[kind] {
                UnionChoice::Ambiguous => Err((
                    path.to_owned(),
                    format!("Ambiguous union target for JSON {}", kind_name(&node)),
                )),
                UnionChoice::Target(child) => {
                    if kind == 0 {
                        Ok(Value::Nil)
                    } else {
                        decode_node(node, plan, child, path, resolve)
                    }
                }
                UnionChoice::Missing => {
                    if let Some(child) = union.dynamic_fallback {
                        decode_node(node, plan, child, path, resolve)
                    } else {
                        Err((
                            path.to_owned(),
                            format!(
                                "JSON {} does not match any variant of target union",
                                kind_name(&node)
                            ),
                        ))
                    }
                }
            }
        }
        PlanNode::Unavailable(type_id) => Err((
            path.to_owned(),
            format!("Type '{}' cannot be decoded from JSON", type_id),
        )),
        PlanNode::Unsupported(ty) => Err((
            path.to_owned(),
            format!("Unsupported target type '{}' for json.decode", ty),
        )),
        PlanNode::InvalidMap(message) => Err((path.to_owned(), message.clone())),
        PlanNode::Deferred(ty) => {
            let deferred = JsonDecodePlan::compile(ty, resolve);
            decode_borrowed_json(node, &deferred, path, resolve)
        }
    }
}

fn find_field_index(schema: &StructPlan, key: &str, cursor: &mut usize) -> Option<usize> {
    if !schema.has_duplicate_json_names {
        if let Some(field) = schema.fields.get(*cursor) {
            if field.json_name == key {
                let idx = *cursor;
                *cursor += 1;
                return Some(idx);
            }
        }
    }
    let found = if schema.fields.len() <= 8 {
        if schema.has_duplicate_json_names {
            schema.fields.iter().rposition(|field| field.json_name == key)
        } else {
            schema.fields.iter().position(|field| field.json_name == key)
        }
    } else {
        schema.field_indexes.get(key).copied()
    };
    if let Some(idx) = found {
        *cursor = idx + 1;
    }
    found
}

fn decode_struct(
    node: BorrowedJsonNode<'_>,
    schema: &StructPlan,
    plan: &JsonDecodePlan,
    path: &str,
    resolve: &ResolveStruct<'_>,
) -> Result<Value, (String, String)> {
    let entries = match node {
        BorrowedJsonNode::Object(entries) => entries,
        other => {
            return Err((
                path.to_owned(),
                format!(
                    "Expected JSON object for '{}', got {}",
                    schema.struct_schema.type_id,
                    kind_name(&other)
                ),
            ));
        }
    };
    let count = schema.fields.len();
    let mut stack: [_; 16] = std::array::from_fn(|_| None);
    let mut heap;
    let slots = if count <= stack.len() {
        &mut stack[..count]
    } else {
        heap = (0..count).map(|_| None).collect::<Vec<_>>();
        &mut heap[..]
    };

    let mut cursor = 0;
    for (key, value) in entries {
        let Some(index) = find_field_index(schema, key.as_ref(), &mut cursor) else {
            return Err((
                join_field_path(path, &key),
                format!(
                    "Unknown field '{}' for type '{}'",
                    key, schema.struct_schema.type_id
                ),
            ));
        };
        slots[index] = Some(value);
    }
    let mut values = Vec::with_capacity(count);
    for (index, field) in schema.fields.iter().enumerate() {
        let value = if schema.has_duplicate_json_names {
            let mut last_cursor = 0;
            let last_idx = find_field_index(schema, &field.json_name, &mut last_cursor).unwrap_or(index);
            slots[last_idx].clone()
        } else {
            slots[index].take()
        };
        if let Some(value) = value {
            let decoded = decode_node(value, plan, field.child, "", resolve).map_err(
                |(child, message)| {
                    (
                        prepend_error_path(join_field_path(path, &field.json_name), &child),
                        message,
                    )
                },
            )?;
            values.push(decoded);
        } else if field.is_optional {
            values.push(Value::Nil);
        } else {
            return Err((
                join_field_path(path, &field.json_name),
                format!("Missing required field '{}'", field.json_name),
            ));
        }
    }
    Ok(Value::Struct {
        schema: Arc::clone(&schema.struct_schema),
        values,
    })
}

fn kind_name(node: &BorrowedJsonNode<'_>) -> &'static str {
    match node {
        BorrowedJsonNode::Null => "null",
        BorrowedJsonNode::Bool(_) => "bool",
        BorrowedJsonNode::Number(_) => "number",
        BorrowedJsonNode::String(_) => "string",
        BorrowedJsonNode::Array(_) => "array",
        BorrowedJsonNode::Object(_) => "object",
    }
}

fn union_kind(node: &BorrowedJsonNode<'_>) -> usize {
    match node {
        BorrowedJsonNode::Null => 0,
        BorrowedJsonNode::Bool(_) => 1,
        BorrowedJsonNode::String(_) => 2,
        BorrowedJsonNode::Number(number) if number.is_int_syntax => 3,
        BorrowedJsonNode::Number(_) => 4,
        BorrowedJsonNode::Array(_) => 5,
        BorrowedJsonNode::Object(_) => 6,
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::cell::Cell;

    use super::*;
    use crate::native::json::parser::parse_borrowed_json;

    fn decode_raw(raw: &str, ty: &Type, resolve: &ResolveStruct<'_>) -> Result<Value, (String, String)> {
        let plan = JsonDecodePlan::compile(ty, resolve);
        let borrowed = parse_borrowed_json(raw)?;
        decode_borrowed_json(borrowed, &plan, "payload", resolve)
    }

    #[test]
    fn compiled_decoder_handles_scalars_containers_and_unions() {
        let dynamic = Type::Custom(TypeId::native("cdt.json", "Value"));
        let no_structs = |_: &TypeId, _: &[Type]| None;

        assert_eq!(decode_raw("7", &Type::Int, &no_structs), Ok(Value::Int(7)));
        assert_eq!(
            decode_raw("-9223372036854775808", &Type::Int, &no_structs),
            Ok(Value::Int(i64::MIN))
        );
        assert!(decode_raw("9223372036854775808", &Type::Int, &no_structs).is_err());
        assert!(decode_raw("7.0", &Type::Int, &no_structs).is_err());
        assert_eq!(
            decode_raw("1e2", &Type::Float, &no_structs),
            Ok(Value::Float(100.0))
        );
        assert_eq!(
            decode_raw("true", &Type::Bool, &no_structs),
            Ok(Value::Bool(true))
        );
        assert_eq!(
            decode_raw(r#""\\n\\u263a""#, &Type::String, &no_structs),
            Ok(Value::String("\\n\\u263a".to_owned()))
        );
        assert_eq!(
            decode_raw("null", &Type::Optional(Box::new(Type::Int)), &no_structs),
            Ok(Value::Nil)
        );
        assert_eq!(
            decode_raw("[1]", &Type::List(Box::new(Type::Int)), &no_structs),
            Ok(Value::List(vec![Value::Int(1)]))
        );
        assert!(decode_raw("[null]", &Type::List(Box::new(Type::Void)), &no_structs).is_err());

        assert_eq!(
            decode_raw(
                r#"{"a":false,"a":7}"#,
                &Type::Map(Box::new(Type::String), Box::new(Type::Int)),
                &no_structs,
            ),
            Err(("payload[\"a\"]".to_owned(), "Expected Int, got bool".to_owned()))
        );
        let mut expected_map = OrderedMap::new();
        expected_map.set(MapKey::String("a".to_owned()), Value::Int(7));
        assert_eq!(
            decode_raw(
                r#"{"a":1,"a":7}"#,
                &Type::Map(Box::new(Type::String), Box::new(Type::Int)),
                &no_structs,
            ),
            Ok(Value::Map(expected_map))
        );
        assert!(decode_raw("{}", &Type::Map(Box::new(Type::Bool), Box::new(Type::String)), &no_structs).is_err());

        assert_eq!(
            decode_raw(
                r#"[1,"s"]"#,
                &Type::Tuple(vec![Type::Int, Type::String]),
                &no_structs,
            ),
            Ok(Value::Tuple(vec![Value::Int(1), Value::String("s".to_owned())]))
        );

        let scalar_union = Type::union(vec![Type::Int, Type::Float, Type::String, Type::Nil]);
        assert_eq!(decode_raw("7", &scalar_union, &no_structs), Ok(Value::Int(7)));
        assert_eq!(decode_raw("7.0", &scalar_union, &no_structs), Ok(Value::Float(7.0)));
        assert_eq!(
            decode_raw("null", &scalar_union, &no_structs),
            Ok(Value::Nil)
        );

        let ambiguous_bool = Type::Union(vec![Type::Bool, Type::Optional(Box::new(Type::Bool))]);
        assert!(decode_raw("true", &ambiguous_bool, &no_structs).is_err());

        let ambiguous_arr = Type::Union(vec![Type::List(Box::new(Type::Int)), Type::Tuple(vec![])]);
        assert!(decode_raw("[]", &ambiguous_arr, &no_structs).is_err());

        let with_dyn = Type::union(vec![Type::Int, dynamic]);
        let dyn_res = decode_raw(r#""text""#, &with_dyn, &no_structs).unwrap();
        assert!(matches!(dyn_res, Value::Native(_)));
        assert!(decode_raw("true", &Type::Custom(TypeId::program("Missing")), &no_structs).is_err());
    }

    #[test]
    fn struct_slots_preserve_order_duplicates_and_unvisited_schema_failures() {
        let tagged_id = TypeId::program("Tagged");
        let tagged = Type::Custom(tagged_id.clone());
        let missing = Type::Custom(TypeId::program("Missing"));
        let resolve = |id: &TypeId, _: &[Type]| {
            (id == &tagged_id).then(|| {
                vec![
                    ("x".to_owned(), "a.b".to_owned(), Type::Int),
                    (
                        "optional".to_owned(),
                        "optional".to_owned(),
                        Type::Optional(Box::new(missing.clone())),
                    ),
                    (
                        "empty".to_owned(),
                        "empty".to_owned(),
                        Type::List(Box::new(Type::Void)),
                    ),
                ]
            })
        };

        let val = decode_raw(r#"{"a.b":false,"a.b":7,"empty":[]}"#, &tagged, &resolve).unwrap();
        assert_eq!(val.field("x"), Some(&Value::Int(7)));
        assert_eq!(val.field("optional"), Some(&Value::Nil));
        assert_eq!(val.field("empty"), Some(&Value::List(vec![])));

        assert_eq!(
            decode_raw(r#"{"a.b":7,"a.b":false,"empty":[]}"#, &tagged, &resolve)
                .unwrap_err()
                .0,
            "payload[\"a.b\"]"
        );
        let err = decode_raw(r#"{"a.b":false,"unknown":0}"#, &tagged, &resolve).unwrap_err();
        assert_eq!(err.0, "payload.unknown");
        assert!(err.1.contains("Unknown field"));
        assert!(decode_raw(r#"{"a.b":7,"optional":{},"empty":[]}"#, &tagged, &resolve).is_err());

        let many = |_: &TypeId, _: &[Type]| {
            Some(
                (0..20)
                    .map(|i| (format!("f{i}"), format!("key{i}"), Type::Int))
                    .collect(),
            )
        };
        let mut entries: Vec<_> = (0..20).map(|i| format!("\"key{i}\":{i}")).collect();
        let raw = format!("{{{}}}", entries.join(","));
        let many_val = decode_raw(&raw, &tagged, &many).unwrap();
        assert_eq!(many_val.field("f19"), Some(&Value::Int(19)));
        entries[19] = "\"unknown\":false".to_owned();
        let err = decode_raw(&format!("{{{}}}", entries.join(",")), &tagged, &many).unwrap_err();
        assert_eq!(err.0, "payload.unknown");
    }

    #[test]
    fn recursive_plans_compile_finitely_and_move_owned_string_buffers() {
        let node_id = TypeId::program("Node");
        let node_ty = Type::Custom(node_id.clone());
        let calls = Cell::new(0);
        let resolve = |id: &TypeId, _: &[Type]| {
            calls.set(calls.get() + 1);
            (id == &node_id).then(|| {
                vec![
                    ("value".to_owned(), "value".to_owned(), Type::Int),
                    (
                        "next".to_owned(),
                        "next".to_owned(),
                        Type::Optional(Box::new(node_ty.clone())),
                    ),
                ]
            })
        };
        let plan = JsonDecodePlan::compile(&node_ty, &resolve);
        assert_eq!(calls.get(), 1);
        let raw = r#"{"value":1,"next":{"value":2,"next":null}}"#;
        let value =
            decode_borrowed_json(parse_borrowed_json(raw).unwrap(), &plan, "", &resolve).unwrap();
        assert!(matches!(value, Value::Struct { .. }));
        assert_eq!(
            calls.get(),
            1,
            "recursive traversal should use arena indexes"
        );

        let expanding_id = TypeId::program("Expanding");
        let expanding = Type::Generic {
            type_id: expanding_id.clone(),
            type_args: vec![Type::Int],
        };
        let resolve = |id: &TypeId, arguments: &[Type]| {
            (id == &expanding_id).then(|| {
                vec![
                    ("value".to_owned(), "value".to_owned(), arguments[0].clone()),
                    (
                        "next".to_owned(),
                        "next".to_owned(),
                        Type::Optional(Box::new(Type::Generic {
                            type_id: id.clone(),
                            type_args: vec![Type::List(Box::new(arguments[0].clone()))],
                        })),
                    ),
                ]
            })
        };
        let exp_val = decode_raw(
            r#"{"value":1,"next":{"value":[2,3],"next":null}}"#,
            &expanding,
            &resolve,
        )
        .unwrap();
        assert_eq!(exp_val.field("value"), Some(&Value::Int(1)));
        let nested = exp_val.field("next").unwrap();
        assert_eq!(
            nested.field("value"),
            Some(&Value::List(vec![Value::Int(2), Value::Int(3)]))
        );
        assert_eq!(nested.field("next"), Some(&Value::Nil));

        let text = "owned escaped string".to_owned();
        let original_buffer = text.as_ptr();
        let plan = JsonDecodePlan::compile(&Type::String, &|_, _| None);
        let result = decode_borrowed_json(
            BorrowedJsonNode::String(Cow::Owned(text)),
            &plan,
            "",
            &|_, _| None,
        )
        .unwrap();
        let Value::String(text) = result else {
            panic!("expected a string")
        };
        assert_eq!(text.as_ptr(), original_buffer);
    }
}
