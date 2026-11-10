use crate::ast::TypeExpr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Int,
    String,
    Bool,
    Nil,
    Void,
    List(Box<Type>),
    Optional(Box<Type>),
    Custom(String),
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
            Type::Any => write!(f, "Any"),
        }
    }
}

impl Type {
    pub fn is_assignable_to(&self, target: &Type) -> bool {
        if target == &Type::Any || self == &Type::Any {
            return true;
        }

        if self == target {
            return true;
        }

        if self == &Type::Nil {
            if let Type::Optional(_) = target {
                return true;
            }
        }

        if let Type::Optional(inner) = target {
            return self.is_assignable_to(inner);
        }

        if let (Type::List(src), Type::List(_dst)) = (self, target) {
            if **src == Type::Nil || **src == Type::Any {
                return true;
            }
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
        }
    }
}

