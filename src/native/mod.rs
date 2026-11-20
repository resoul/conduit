pub(crate) mod b64_simd;
pub mod cache;
pub mod cli;
pub mod cron;
pub mod crypto;
pub mod csv;
pub mod db;
pub mod db_pool;
pub mod db_query;
pub mod env;
pub mod fs;
pub mod http;
pub mod json_decode;
pub mod json_encode;
pub(crate) mod json_lexer;
pub mod jwt;
pub mod lock;
pub mod log;
pub mod math;
pub mod mysql;
pub mod path;
pub mod pg;
pub mod process;
pub mod redis;
pub mod time;
pub mod url;
pub mod validation;

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use crate::vm::value::FxBuildHasher;

pub const MAX_JSON_DEPTH: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    pub path: String,
    pub message: String,
}

impl JsonError {
    #[cold]
    #[inline(never)]
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }

    #[cold]
    #[inline(never)]
    pub fn root(message: impl Into<String>) -> Self {
        Self {
            path: String::new(),
            message: message.into(),
        }
    }
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarKind {
    Int,
    Float,
    Bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum JsonType {
    Int,
    Float,
    Bool,
    String,
    Optional(Box<JsonType>),
    List(Box<JsonType>),
    Map(Box<JsonType>),
    Struct(u32),
    TaggedEnum {
        name: String,
        variants: Vec<(String, u32)>,
    },
}

impl JsonType {
    pub fn display_name(&self, schemas: &[StructSchema]) -> String {
        match self {
            JsonType::Int => "Int".to_string(),
            JsonType::Float => "Float".to_string(),
            JsonType::Bool => "Bool".to_string(),
            JsonType::String => "String".to_string(),
            JsonType::Optional(inner) => format!("{}?", inner.display_name(schemas)),
            JsonType::List(inner) => format!("[{}]", inner.display_name(schemas)),
            JsonType::Map(val) => format!("Map<String, {}>", val.display_name(schemas)),
            JsonType::Struct(id) => find_schema(schemas, *id)
                .map(|s| s.type_name.clone())
                .unwrap_or_else(|| format!("Struct#{}", id)),
            JsonType::TaggedEnum { name, .. } => name.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StructFieldDef {
    pub name: String,
    pub json_name: String,
    pub ty: JsonType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldSlot {
    Scalar { kind: ScalarKind, slot: u16 },
    Obj { slot: u16, is_optional: bool },
}

#[derive(Debug, Clone)]
pub struct CompiledStructField {
    pub name: String,
    pub json_name: String,
    pub ty: JsonType,
    pub is_optional: bool,
    pub slot: FieldSlot,
    pub wire_prefix: Box<[u8]>,
    pub quoted_key_colon: Box<[u8]>,
}

#[derive(Debug, Clone)]
pub struct StructSchema {
    pub schema_id: u32,
    pub type_name: String,
    pub scalar_len: u16,
    pub obj_len: u16,
    pub fields: Box<[CompiledStructField]>,
    pub has_duplicate_json_names: bool,
    pub is_error: bool,
    pub enum_tag: Option<String>,
}

impl StructSchema {
    pub fn new(
        schema_id: u32,
        type_name: impl Into<String>,
        field_defs: Vec<StructFieldDef>,
    ) -> Self {
        Self::with_flags(schema_id, type_name, field_defs, false, None)
    }

    pub fn with_error_flag(
        schema_id: u32,
        type_name: impl Into<String>,
        field_defs: Vec<StructFieldDef>,
        is_error: bool,
    ) -> Self {
        Self::with_flags(schema_id, type_name, field_defs, is_error, None)
    }

    pub fn with_flags(
        schema_id: u32,
        type_name: impl Into<String>,
        field_defs: Vec<StructFieldDef>,
        is_error: bool,
        enum_tag: Option<String>,
    ) -> Self {
        let mut scalar_len: u16 = 0;
        let mut obj_len: u16 = 0;
        let mut seen = Vec::with_capacity(field_defs.len());
        let mut has_duplicate_json_names = false;
        let mut fields = Vec::with_capacity(field_defs.len());

        for (index, def) in field_defs.into_iter().enumerate() {
            has_duplicate_json_names |= seen.iter().any(|prev| prev == &def.json_name);
            seen.push(def.json_name.clone());

            let is_optional = matches!(def.ty, JsonType::Optional(_));
            let slot = match def.ty {
                JsonType::Int => {
                    let s = scalar_len;
                    scalar_len += 1;
                    FieldSlot::Scalar {
                        kind: ScalarKind::Int,
                        slot: s,
                    }
                }
                JsonType::Float => {
                    let s = scalar_len;
                    scalar_len += 1;
                    FieldSlot::Scalar {
                        kind: ScalarKind::Float,
                        slot: s,
                    }
                }
                JsonType::Bool => {
                    let s = scalar_len;
                    scalar_len += 1;
                    FieldSlot::Scalar {
                        kind: ScalarKind::Bool,
                        slot: s,
                    }
                }
                _ => {
                    let s = obj_len;
                    obj_len += 1;
                    FieldSlot::Obj {
                        slot: s,
                        is_optional,
                    }
                }
            };

            let mut quoted_key_colon = Vec::with_capacity(def.json_name.len() + 4);
            json_encode::write_escaped_bytes(&def.json_name, &mut quoted_key_colon);
            quoted_key_colon.push(b':');

            let mut wire_prefix = Vec::with_capacity(quoted_key_colon.len() + 1);
            wire_prefix.push(if index == 0 { b'{' } else { b',' });
            wire_prefix.extend_from_slice(&quoted_key_colon);

            fields.push(CompiledStructField {
                name: def.name,
                json_name: def.json_name,
                ty: def.ty,
                is_optional,
                slot,
                wire_prefix: wire_prefix.into_boxed_slice(),
                quoted_key_colon: quoted_key_colon.into_boxed_slice(),
            });
        }

        Self {
            schema_id,
            type_name: type_name.into(),
            scalar_len,
            obj_len,
            fields: fields.into_boxed_slice(),
            has_duplicate_json_names,
            is_error,
            enum_tag,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CompiledStructDecodePlan {
    pub schema_id: u32,
    pub type_name: String,
    pub scalar_len: u16,
    pub obj_len: u16,
    pub fields: Box<[DecodedFieldPlan]>,
    pub field_indexes: HashMap<String, usize, FxBuildHasher>,
    pub required_mask: u64,
    pub has_duplicate_json_names: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct DecodedFieldPlan {
    pub json_name: String,
    pub quoted_key_colon: Box<[u8]>,
    pub is_optional: bool,
    pub kind: FieldDecodeKind,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum FieldDecodeKind {
    Int(u16),
    Float(u16),
    Bool(u16),
    Str(u16),
    OptStr(u16),
    Obj { slot: u16, plan_idx: usize },
}

#[derive(Debug, Clone)]
pub(crate) enum PlanNode {
    Pending,
    Int,
    Float,
    Bool,
    String,
    Optional(usize),
    ListInt,
    ListFloat,
    ListStr,
    ListObj(usize),
    Map {
        val_plan: usize,
        val_name: String,
    },
    Struct(CompiledStructDecodePlan),
    TaggedEnum {
        type_name: String,
        variants: Box<[(String, CompiledStructDecodePlan)]>,
    },
    Unavailable(u32),
}

#[derive(Debug, Clone)]
pub struct JsonDecodePlan {
    pub root_ty: JsonType,
    pub(crate) nodes: Vec<PlanNode>,
    pub(crate) root: usize,
}

impl JsonDecodePlan {
    pub fn compile(root_ty: &JsonType, schemas: &[StructSchema]) -> Self {
        let mut compiler = PlanCompiler {
            nodes: Vec::new(),
            indexes: BTreeMap::new(),
            schemas,
        };
        let root = compiler.add(root_ty);
        Self {
            root_ty: root_ty.clone(),
            nodes: compiler.nodes,
            root,
        }
    }
}

struct PlanCompiler<'a> {
    nodes: Vec<PlanNode>,
    indexes: BTreeMap<JsonType, usize>,
    schemas: &'a [StructSchema],
}

impl PlanCompiler<'_> {
    fn add(&mut self, ty: &JsonType) -> usize {
        if let Some(&idx) = self.indexes.get(ty) {
            return idx;
        }
        let idx = self.nodes.len();
        self.nodes.push(PlanNode::Pending);
        self.indexes.insert(ty.clone(), idx);

        let node = match ty {
            JsonType::Int => PlanNode::Int,
            JsonType::Float => PlanNode::Float,
            JsonType::Bool => PlanNode::Bool,
            JsonType::String => PlanNode::String,
            JsonType::Optional(inner) => PlanNode::Optional(self.add(inner)),
            JsonType::List(inner) => match inner.as_ref() {
                JsonType::Int => PlanNode::ListInt,
                JsonType::Float => PlanNode::ListFloat,
                JsonType::String => PlanNode::ListStr,
                _ => PlanNode::ListObj(self.add(inner)),
            },
            JsonType::Map(val) => PlanNode::Map {
                val_plan: self.add(val),
                val_name: val.display_name(self.schemas),
            },
            JsonType::Struct(id) => self.compile_struct(*id),
            JsonType::TaggedEnum { name, variants } => {
                let mut compiled = Vec::with_capacity(variants.len());
                let mut missing_sid = None;
                for (vname, sid) in variants {
                    if let PlanNode::Struct(plan) = self.compile_struct(*sid) {
                        compiled.push((vname.clone(), plan));
                    } else {
                        missing_sid = Some(*sid);
                        break;
                    }
                }
                if let Some(sid) = missing_sid {
                    PlanNode::Unavailable(sid)
                } else {
                    PlanNode::TaggedEnum {
                        type_name: name.clone(),
                        variants: compiled.into_boxed_slice(),
                    }
                }
            }
        };
        self.nodes[idx] = node;
        idx
    }

    fn compile_struct(&mut self, schema_id: u32) -> PlanNode {
        let Some(schema) = find_schema(self.schemas, schema_id) else {
            return PlanNode::Unavailable(schema_id);
        };
        let use_map = schema.fields.len() > 8;
        let mut field_indexes = if use_map {
            HashMap::with_capacity_and_hasher(schema.fields.len(), FxBuildHasher)
        } else {
            HashMap::with_hasher(FxBuildHasher)
        };
        let mut decoded_fields = Vec::with_capacity(schema.fields.len());
        let mut required_mask = 0u64;

        for (index, field) in schema.fields.iter().enumerate() {
            if use_map {
                field_indexes.insert(field.json_name.clone(), index);
            }
            if !field.is_optional && index < 64 {
                required_mask |= 1u64 << index;
            }
            let kind = match (field.slot, &field.ty) {
                (FieldSlot::Scalar { kind: ScalarKind::Int, slot }, _) => FieldDecodeKind::Int(slot),
                (FieldSlot::Scalar { kind: ScalarKind::Float, slot }, _) => {
                    FieldDecodeKind::Float(slot)
                }
                (FieldSlot::Scalar { kind: ScalarKind::Bool, slot }, _) => {
                    FieldDecodeKind::Bool(slot)
                }
                (FieldSlot::Obj { slot, .. }, JsonType::String) => FieldDecodeKind::Str(slot),
                (FieldSlot::Obj { slot, .. }, JsonType::Optional(inner))
                    if matches!(inner.as_ref(), JsonType::String) =>
                {
                    FieldDecodeKind::OptStr(slot)
                }
                (FieldSlot::Obj { slot, .. }, _) => FieldDecodeKind::Obj {
                    slot,
                    plan_idx: self.add(&field.ty),
                },
            };
            decoded_fields.push(DecodedFieldPlan {
                json_name: field.json_name.clone(),
                quoted_key_colon: field.quoted_key_colon.clone(),
                is_optional: field.is_optional,
                kind,
            });
        }

        PlanNode::Struct(CompiledStructDecodePlan {
            schema_id: schema.schema_id,
            type_name: schema.type_name.clone(),
            scalar_len: schema.scalar_len,
            obj_len: schema.obj_len,
            fields: decoded_fields.into_boxed_slice(),
            field_indexes,
            required_mask,
            has_duplicate_json_names: schema.has_duplicate_json_names,
        })
    }
}

#[inline]
pub(crate) fn find_schema(schemas: &[StructSchema], schema_id: u32) -> Option<&StructSchema> {
    schemas
        .get(schema_id as usize)
        .filter(|s| s.schema_id == schema_id)
        .or_else(|| schemas.iter().find(|s| s.schema_id == schema_id))
}

#[cold]
#[inline(never)]
pub fn join_field_path(base: &str, field: &str) -> String {
    let mut chars = field.chars();
    let is_identifier = chars
        .next()
        .is_some_and(|ch| ch.is_alphabetic() || ch == '_')
        && chars.all(|ch| ch.is_alphanumeric() || ch == '_');
    if !is_identifier {
        let mut buf = Vec::with_capacity(base.len() + field.len() + 4);
        buf.extend_from_slice(base.as_bytes());
        buf.push(b'[');
        json_encode::write_escaped_bytes(field, &mut buf);
        buf.push(b']');
        return String::from_utf8(buf).expect("escaped path must be UTF-8");
    }
    if base.is_empty() {
        field.to_string()
    } else {
        format!("{base}.{field}")
    }
}

#[cold]
#[inline(never)]
pub fn join_index_path(base: &str, idx: usize) -> String {
    format!("{base}[{idx}]")
}

#[cold]
#[inline(never)]
pub fn prepend_error_path(mut prefix: String, child: &str) -> String {
    if !child.is_empty() {
        if !prefix.is_empty() && !child.starts_with('[') {
            prefix.push('.');
        }
        prefix.push_str(child);
    }
    prefix
}

#[cold]
#[inline(never)]
pub(crate) fn err_at_index(idx: usize, err: JsonError) -> JsonError {
    JsonError {
        path: prepend_error_path(join_index_path("", idx), &err.path),
        message: err.message,
    }
}

#[cold]
#[inline(never)]
pub(crate) fn err_at_field(field: &str, err: JsonError) -> JsonError {
    JsonError {
        path: prepend_error_path(join_field_path("", field), &err.path),
        message: err.message,
    }
}
