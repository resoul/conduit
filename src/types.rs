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

        if let (Type::Custom(s), Type::Custom(d)) = (self, target) {
            let s_item = s.split('.').last().unwrap_or(s);
            let d_item = d.split('.').last().unwrap_or(d);
            if s_item == d_item {
                let s_mod = s.split_once('.').map(|x| x.0);
                let d_mod = d.split_once('.').map(|x| x.0);
                match (s_mod, d_mod) {
                    (None, _) | (_, None) => return true,
                    (Some(m1), Some(m2)) => {
                        if m1 == m2 {
                            return true;
                        }
                        let time_aliases = ["time", "clock"];
                        if time_aliases.contains(&m1) && time_aliases.contains(&m2) {
                            return true;
                        }
                    }
                }
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
            TypeExpr::Func { params, return_type } => {
                let p = params.iter().map(Self::from_type_expr).collect();
                let r = Box::new(Self::from_type_expr(return_type));
                Type::Function {
                    params: p,
                    return_type: r,
                }
            }
        }
    }
}
