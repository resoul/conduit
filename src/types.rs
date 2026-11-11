use std::path::PathBuf;

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
    pub fn builtin(name: impl Into<String>) -> Self {
        Self {
            module: ModuleId::Builtin,
            name: name.into(),
        }
    }

    pub fn program(name: impl Into<String>) -> Self {
        Self {
            module: ModuleId::Program,
            name: name.into(),
        }
    }

    pub fn local(path: impl Into<PathBuf>, name: impl Into<String>) -> Self {
        Self {
            module: ModuleId::Local(path.into()),
            name: name.into(),
        }
    }

    pub fn native(module: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            module: ModuleId::Native(module.into()),
            name: name.into(),
        }
    }

    pub fn from_module_opt(module: Option<&PathBuf>, name: impl Into<String>) -> Self {
        match module {
            Some(path) => Self::local(path.clone(), name),
            None => Self::program(name),
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Type {
    Int,
    String,
    Bool,
    Nil,
    Void,
    List(Box<Type>),
    Optional(Box<Type>),
    Custom(TypeId),
    Tuple(Vec<Type>),
    Union(Vec<Type>),
    Function {
        params: Vec<Type>,
        return_type: Box<Type>,
    },
    Any,
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Int => write!(f, "Int"),
            Type::String => write!(f, "String"),
            Type::Bool => write!(f, "Bool"),
            Type::Nil => write!(f, "Nil"),
            Type::Void => write!(f, "Void"),
            Type::List(inner) => write!(f, "[{}]", inner),
            Type::Optional(inner) => write!(f, "{}?", inner),
            Type::Custom(name) => write!(f, "{}", name),
            Type::Tuple(elems) => {
                let s = elems.iter().map(|e| e.to_string()).collect::<Vec<_>>().join(", ");
                write!(f, "({})", s)
            }
            Type::Union(vars) => {
                let s = vars.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(" | ");
                write!(f, "{}", s)
            }
            Type::Function { params, return_type } => {
                let s = params.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ");
                write!(f, "({}) -> {}", s, return_type)
            }
            Type::Any => write!(f, "Any"),
        }
    }
}

impl Type {
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

        if let Type::Optional(inner) = self {
            if let Type::Union(_) = target {
                return inner.is_assignable_to(target) && Type::Nil.is_assignable_to(target);
            }
        }

        if let Type::Union(src_variants) = self {
            return src_variants.iter().all(|v| v.is_assignable_to(target));
        }

        if let Type::Union(dst_variants) = target {
            return dst_variants.iter().any(|v| self.is_assignable_to(v));
        }

        if self == &Type::Nil {
            if let Type::Optional(_) = target {
                return true;
            }
        }

        if let Type::Optional(inner) = target {
            return self.is_assignable_to(inner);
        }

        if let (Type::Tuple(src_elems), Type::Tuple(dst_elems)) = (self, target) {
            if src_elems.len() != dst_elems.len() {
                return false;
            }
            return src_elems.iter().zip(dst_elems.iter()).all(|(s, d)| s.is_assignable_to(d));
        }

        if let (Type::List(src), Type::List(dst)) = (self, target) {
            if **src == Type::Nil || **src == Type::Any {
                return true;
            }
            return src.is_assignable_to(dst);
        }

        if let (
            Type::Function { params: src_p, return_type: src_r },
            Type::Function { params: dst_p, return_type: dst_r },
        ) = (self, target) {
            if src_p.len() != dst_p.len() {
                return false;
            }
            let params_ok = src_p.iter().zip(dst_p.iter()).all(|(sp, dp)| dp.is_assignable_to(sp));
            let ret_ok = src_r.is_assignable_to(dst_r);
            return params_ok && ret_ok;
        }

        false
    }
}
