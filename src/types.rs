use crate::ast::TypeExpr;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Type {
    Int,
    String,
    Bool,
    Nil,
    Void,
    List(Box<Type>),
    Optional(Box<Type>),
    Custom(String),
    Tuple(Vec<Type>),
    Union(Vec<Type>),
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

        false
    }

    pub fn from_type_expr(expr: &TypeExpr) -> Self {
        match expr {
            TypeExpr::Named(name) => match name.as_str() {
                "Int" => Type::Int,
                "String" => Type::String,
                "Bool" => Type::Bool,
                "Void" => Type::Void,
                custom => Type::Custom(custom.to_string()),
            },
            TypeExpr::List(inner) => Type::List(Box::new(Self::from_type_expr(inner))),
            TypeExpr::Optional(inner) => Type::Optional(Box::new(Self::from_type_expr(inner))),
            TypeExpr::Union(variants) => {
                let types = variants.iter().map(Self::from_type_expr).collect();
                Self::union(types)
            }
            TypeExpr::Tuple(elements) => {
                let types = elements.iter().map(Self::from_type_expr).collect();
                Type::Tuple(types)
            }
        }
    }
}

