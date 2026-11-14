use std::path::PathBuf;

use crate::ir::ElemKind;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ModuleId {
    Builtin,
    Program,
    Local(PathBuf),
    Native(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeId {
    pub module: ModuleId,
    pub name: String,
}

impl TypeId {
    pub fn program(name: impl Into<String>) -> Self {
        Self {
            module: ModuleId::Program,
            name: name.into(),
        }
    }

    pub fn native(module: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            module: ModuleId::Native(module.into()),
            name: name.into(),
        }
    }
}

impl std::fmt::Display for TypeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.module {
            ModuleId::Builtin | ModuleId::Program | ModuleId::Local(_) => {
                write!(f, "{}", self.name)
            }
            ModuleId::Native(mod_name) => {
                let short = mod_name.strip_prefix("cdt.").unwrap_or(mod_name);
                write!(f, "{}.{}", short, self.name)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Type {
    Int,
    Float,
    String,
    Bool,
    Nil,
    Void,
    TypeParam(String),
    List(Box<Type>),
    Map(Box<Type>, Box<Type>),
    Optional(Box<Type>),
    Custom(TypeId),
    Generic {
        type_id: TypeId,
        type_args: Vec<Type>,
    },
    Tuple(Vec<Type>),
    Union(Vec<Type>),
    Function {
        params: Vec<Type>,
        return_type: Box<Type>,
    },
    #[default]
    Any,
}

impl Type {
    #[inline]
    pub fn is_scalar(&self) -> bool {
        matches!(self, Type::Int | Type::Float | Type::Bool)
    }

    #[inline]
    pub fn is_int(&self) -> bool {
        matches!(self, Type::Int)
    }

    #[inline]
    pub fn is_float(&self) -> bool {
        matches!(self, Type::Float)
    }

    #[inline]
    pub fn is_bool(&self) -> bool {
        matches!(self, Type::Bool)
    }

    #[inline]
    pub fn is_obj(&self) -> bool {
        !self.is_scalar() && !matches!(self, Type::Void)
    }

    #[inline]
    pub fn elem_kind(&self) -> ElemKind {
        match self {
            Type::Int => ElemKind::Int,
            Type::Float => ElemKind::Float,
            _ => ElemKind::Obj,
        }
    }

    pub fn union(variants: Vec<Type>) -> Type {
        let mut flat = Vec::new();
        for v in variants {
            match v {
                Type::Union(inner) => {
                    for iv in inner {
                        if let Type::Optional(opt_inner) = iv {
                            flat.push(*opt_inner);
                            flat.push(Type::Nil);
                        } else {
                            flat.push(iv);
                        }
                    }
                }
                Type::Optional(opt_inner) => {
                    flat.push(*opt_inner);
                    flat.push(Type::Nil);
                }
                other => flat.push(other),
            }
        }
        flat.sort();
        flat.dedup();
        if flat.len() == 1 {
            flat.pop().unwrap()
        } else {
            Type::Union(flat)
        }
    }

    pub fn is_assignable_to(&self, target: &Type) -> bool {
        if target == &Type::Any || self == &Type::Any {
            return true;
        }
        if self == target {
            return true;
        }
        if let Type::Optional(inner) = self
            && let Type::Union(_) = target
        {
            return inner.is_assignable_to(target) && Type::Nil.is_assignable_to(target);
        }
        if let Type::Union(src_variants) = self {
            return src_variants.iter().all(|v| v.is_assignable_to(target));
        }
        if let Type::Union(dst_variants) = target {
            return dst_variants.iter().any(|v| self.is_assignable_to(v));
        }
        if self == &Type::Nil
            && let Type::Optional(_) = target
        {
            return true;
        }
        if let Type::Optional(inner) = target {
            return self.is_assignable_to(inner);
        }
        if let (Type::Tuple(src_elems), Type::Tuple(dst_elems)) = (self, target) {
            return src_elems.len() == dst_elems.len()
                && src_elems
                    .iter()
                    .zip(dst_elems.iter())
                    .all(|(s, d)| s.is_assignable_to(d));
        }
        if let (Type::List(src), Type::List(dst)) = (self, target) {
            if **src == Type::Nil || **src == Type::Any {
                return true;
            }
            return src.is_assignable_to(dst);
        }
        if let (Type::Map(sk, sv), Type::Map(dk, dv)) = (self, target) {
            if **sk == Type::Any && **sv == Type::Any {
                return true;
            }
            return sk.is_assignable_to(dk) && sv.is_assignable_to(dv);
        }
        if let (
            Type::Generic {
                type_id: src_id,
                type_args: src_args,
            },
            Type::Generic {
                type_id: dst_id,
                type_args: dst_args,
            },
        ) = (self, target)
        {
            return src_id == dst_id && src_args == dst_args;
        }
        if let (
            Type::Function {
                params: src_p,
                return_type: src_r,
            },
            Type::Function {
                params: dst_p,
                return_type: dst_r,
            },
        ) = (self, target)
        {
            if src_p.len() != dst_p.len() {
                return false;
            }
            let params_ok = src_p
                .iter()
                .zip(dst_p.iter())
                .all(|(sp, dp)| dp.is_assignable_to(sp));
            let ret_ok = src_r.is_assignable_to(dst_r);
            return params_ok && ret_ok;
        }
        false
    }
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Int => write!(f, "Int"),
            Type::Float => write!(f, "Float"),
            Type::String => write!(f, "String"),
            Type::Bool => write!(f, "Bool"),
            Type::Nil => write!(f, "Nil"),
            Type::Void => write!(f, "Void"),
            Type::TypeParam(name) => write!(f, "{}", name),
            Type::List(inner) => write!(f, "[{}]", inner),
            Type::Map(key, val) => write!(f, "Map<{}, {}>", key, val),
            Type::Optional(inner) => write!(f, "{}?", inner),
            Type::Custom(name) => write!(f, "{}", name),
            Type::Generic { type_id, type_args } => {
                let s = type_args
                    .iter()
                    .map(|t| t.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "{}<{}>", type_id, s)
            }
            Type::Tuple(elems) => {
                let s = elems
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "({})", s)
            }
            Type::Union(vars) => {
                let s = vars
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(" | ");
                write!(f, "{}", s)
            }
            Type::Function {
                params,
                return_type,
            } => {
                let s = params
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "({}) -> {}", s, return_type)
            }
            Type::Any => write!(f, "Any"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedField {
    pub name: String,
    pub json_name: String,
    pub ty: Type,
    pub is_scalar: bool,
    pub slot_idx: u16,
}

#[derive(Debug, Clone)]
pub struct ResolvedStruct {
    pub schema_id: u32,
    pub name: String,
    pub fields: Vec<ResolvedField>,
    pub scalar_len: u16,
    pub obj_len: u16,
}

impl ResolvedStruct {
    pub fn field(&self, name: &str) -> Option<&ResolvedField> {
        self.fields.iter().find(|f| f.name == name)
    }
}

#[derive(Debug, Clone)]
pub struct FuncSig {
    pub name: String,
    pub params: Vec<Type>,
    pub return_type: Type,
}
