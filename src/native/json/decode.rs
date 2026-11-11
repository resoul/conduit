use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::types::{Type, TypeId};
use crate::value::{MapKey, OrderedMap, Value};

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
    type_id: TypeId,
    fields: Vec<FieldPlan>,
    field_indexes: HashMap<String, usize>,
    has_duplicate_json_names: bool,
}

#[derive(Debug)]
struct FieldPlan {
    name: String,
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
        // Exact recursive types already reuse their pending arena index. A
        // different instantiation can expand forever, so compile it only if its
        // corresponding JSON value is actually visited.
        if self.active_structs.contains(type_id) {
            return PlanNode::Deferred(ty.clone());
        }
        let Some(definitions) = (self.resolve)(type_id, type_args) else {
            return PlanNode::Unavailable(type_id.clone());
        };
        self.active_structs.push(type_id.clone());
        let mut fields = Vec::with_capacity(definitions.len());
        let mut field_indexes = HashMap::with_capacity(definitions.len());
        let mut has_duplicate_json_names = false;
        for (name, json_name, ty) in definitions {
            let index = fields.len();
            has_duplicate_json_names |= field_indexes.insert(json_name.clone(), index).is_some();
            fields.push(FieldPlan {
                name,
                json_name,
                is_optional: union_contains_nil(&ty),
                child: self.add(&ty),
            });
        }
        self.active_structs.pop();
        PlanNode::Struct(StructPlan {
            type_id: type_id.clone(),
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

// Null, bool, string, integer number, other number, array, object. These
// predicates intentionally mirror the legacy union dispatch before conversion.
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
                    schema.type_id,
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
    // Every input key is checked before any field conversion, and later input
    // duplicates overwrite earlier nodes even if those earlier nodes were bad.
    for (key, value) in entries {
        let Some(&index) = schema.field_indexes.get(key.as_ref()) else {
            return Err((
                join_field_path(path, &key),
                format!("Unknown field '{}' for type '{}'", key, schema.type_id),
            ));
        };
        slots[index] = Some(value);
    }
    let mut fields = HashMap::with_capacity(count);
    for (index, field) in schema.fields.iter().enumerate() {
        let value = if schema.has_duplicate_json_names {
            // Only legacy resolver callbacks can create repeated schema names.
            // Each field must receive its own conversion of the same last node.
            slots[schema.field_indexes[&field.json_name]].clone()
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
            fields.insert(field.name.clone(), decoded);
        } else if field.is_optional {
            fields.insert(field.name.clone(), Value::Nil);
        } else {
            return Err((
                join_field_path(path, &field.json_name),
                format!("Missing required field '{}'", field.json_name),
            ));
        }
    }
    Ok(Value::Struct {
        type_id: schema.type_id.clone(),
        fields,
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
    use crate::native::json::decode_node_to_conduit_with_json_names;
    use crate::native::json::parse_json;
    use crate::native::json::parser::parse_borrowed_json;

    fn assert_matches_reference(raw: &str, ty: &Type, resolve: &ResolveStruct<'_>) {
        let owned = parse_json(raw).unwrap();
        let expected = decode_node_to_conduit_with_json_names(&owned, ty, "payload", resolve);
        let plan = JsonDecodePlan::compile(ty, resolve);
        let borrowed = parse_borrowed_json(raw).unwrap();
        let actual = decode_borrowed_json(borrowed, &plan, "payload", resolve);
        assert_eq!(actual, expected, "type={ty}, json={raw}");
    }

    #[test]
    fn compiled_decoder_matches_legacy_scalar_container_and_union_semantics() {
        let dynamic = Type::Custom(TypeId::native("cdt.json", "Value"));
        let types = vec![
            Type::Int,
            Type::Float,
            Type::Bool,
            Type::String,
            Type::Nil,
            Type::Void,
            Type::Any,
            Type::TypeParam("T".to_owned()),
            Type::Custom(TypeId::program("Missing")),
            Type::Optional(Box::new(Type::Int)),
            Type::List(Box::new(Type::Int)),
            Type::List(Box::new(Type::Void)),
            Type::Map(Box::new(Type::String), Box::new(Type::Int)),
            Type::Map(Box::new(Type::Bool), Box::new(Type::String)),
            Type::Tuple(vec![Type::Int, Type::String]),
            Type::Union(vec![Type::Int, Type::Float, Type::String, Type::Nil]),
            Type::Union(vec![Type::Bool, Type::Optional(Box::new(Type::Bool))]),
            Type::Union(vec![Type::List(Box::new(Type::Int)), Type::Tuple(vec![])]),
            Type::Union(vec![Type::Optional(Box::new(Type::List(Box::new(
                Type::Int,
            ))))]),
            Type::Union(vec![Type::Int, dynamic.clone()]),
            Type::Union(vec![
                Type::Custom(TypeId::program("Missing")),
                dynamic.clone(),
            ]),
            dynamic,
        ];
        for raw in [
            "null",
            "true",
            "false",
            "7",
            "-9223372036854775808",
            "9223372036854775808",
            "7.0",
            "1e2",
            r#""text""#,
            r#""\\n\\u263a""#,
            "[]",
            "[1]",
            r#"[1,"s"]"#,
            "{}",
            r#"{"a.b":1}"#,
            r#"{"a":false,"a":7}"#,
        ] {
            for ty in &types {
                assert_matches_reference(raw, ty, &|_, _| None);
            }
        }
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
        for raw in [
            r#"{"a.b":7,"empty":[]}"#,
            r#"{"a.b":7,"optional":null,"empty":[]}"#,
            r#"{"a.b":false,"a.b":7,"empty":[]}"#,
            r#"{"a.b":7,"a.b":false,"empty":[]}"#,
            r#"{"a.b":false,"unknown":0}"#,
            r#"{"unknown":0}"#,
            r#"{"empty":[null]}"#,
            r#"{"a.b":7,"optional":{},"empty":[]}"#,
            "null",
        ] {
            assert_matches_reference(raw, &tagged, &resolve);
        }

        let duplicate_resolver = |_: &TypeId, _: &[Type]| {
            Some(vec![
                ("x".to_owned(), "key".to_owned(), Type::Int),
                ("y".to_owned(), "key".to_owned(), Type::Float),
            ])
        };
        for raw in [
            r#"{"key":7}"#,
            r#"{"key":false,"key":7}"#,
            r#"{"key":"bad"}"#,
            "{}",
        ] {
            assert_matches_reference(raw, &tagged, &duplicate_resolver);
        }

        let many = |_: &TypeId, _: &[Type]| {
            Some(
                (0..20)
                    .map(|i| (format!("f{i}"), format!("key{i}"), Type::Int))
                    .collect(),
            )
        };
        let mut entries: Vec<_> = (0..20).map(|i| format!("\"key{i}\":{i}")).collect();
        let raw = format!("{{{}}}", entries.join(","));
        assert_matches_reference(&raw, &tagged, &many);
        entries[19] = "\"unknown\":false".to_owned();
        assert_matches_reference(&format!("{{{}}}", entries.join(",")), &tagged, &many);
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
        assert_matches_reference(
            r#"{"value":1,"next":{"value":[2,3],"next":null}}"#,
            &expanding,
            &resolve,
        );

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
