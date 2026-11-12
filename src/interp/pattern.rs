use super::*;

impl Interpreter {
    fn resolve_pattern_type(&self, name: &str) -> ResolvedPatternType {
        match name {
            "Int" => ResolvedPatternType::Int,
            "Float" => ResolvedPatternType::Float,
            "String" => ResolvedPatternType::String,
            "Bool" => ResolvedPatternType::Bool,
            "Void" => ResolvedPatternType::Void,
            _ => {
                if let Some((alias, item_name)) = name.split_once('.') {
                    if let Some(target) = self
                        .module_imports
                        .get(&self.current_module)
                        .and_then(|m| m.get(alias))
                    {
                        match target {
                            ModuleTarget::Native(nat_name) => {
                                return ResolvedPatternType::Custom(TypeId::native(
                                    nat_name.clone(),
                                    item_name,
                                ));
                            }
                            ModuleTarget::Local(path) => {
                                return ResolvedPatternType::Custom(TypeId::local(
                                    path.clone(),
                                    item_name,
                                ));
                            }
                        }
                    }
                    ResolvedPatternType::Unknown
                } else if self
                    .module_structs
                    .get(&self.current_module)
                    .is_some_and(|s| s.contains(name))
                    || self
                        .module_errors
                        .get(&self.current_module)
                        .is_some_and(|e| e.contains(name))
                {
                    ResolvedPatternType::Custom(TypeId::from_module_opt(
                        self.current_module.as_ref(),
                        name,
                    ))
                } else if matches!(name, "IndexError" | "KeyError" | "ParseError") {
                    ResolvedPatternType::Custom(TypeId::builtin(name))
                } else {
                    ResolvedPatternType::Custom(TypeId::from_module_opt(
                        self.current_module.as_ref(),
                        name,
                    ))
                }
            }
        }
    }

    pub(super) fn match_pattern(&self, pattern: &Pattern, val: &Value) -> Option<Vec<(String, Value)>> {
        match &pattern.kind {
            PatternKind::Nil => {
                if *val == Value::Nil {
                    Some(Vec::new())
                } else {
                    None
                }
            }
            PatternKind::Type { name, binding } => {
                let target = self.resolve_pattern_type(name);
                let matched = match (val, target) {
                    (
                        Value::Struct { schema, .. } | Value::Error { schema, .. },
                        ResolvedPatternType::Custom(ref pat_id),
                    ) => &schema.type_id == pat_id,
                    (Value::Native(nat), ResolvedPatternType::Custom(ref pat_id)) => {
                        &nat.type_id() == pat_id
                    }
                    (Value::Int(_), ResolvedPatternType::Int) => true,
                    (Value::Float(_), ResolvedPatternType::Float) => true,
                    (Value::String(_), ResolvedPatternType::String) => true,
                    (Value::Bool(_), ResolvedPatternType::Bool) => true,
                    (Value::Void, ResolvedPatternType::Void) => true,
                    _ => false,
                };
                if matched {
                    Some(vec![(binding.clone(), val.clone())])
                } else {
                    None
                }
            }
            PatternKind::Tuple(pats) => {
                if let Value::Tuple(items) = val
                    && pats.len() == items.len() {
                        let mut bindings = Vec::new();
                        for (pat, item) in pats.iter().zip(items) {
                            match &pat.kind {
                                PatternKind::Ident(var_name) => {
                                    bindings.push((var_name.clone(), item.clone()));
                                }
                                PatternKind::Wildcard => {}
                                _ => return None,
                            }
                        }
                        return Some(bindings);
                    }
                None
            }
            PatternKind::Ident(var_name) => {
                Some(vec![(var_name.clone(), val.clone())])
            }
            PatternKind::Wildcard => {
                Some(Vec::new())
            }
        }
    }
}

enum ResolvedPatternType {
    Int,
    Float,
    String,
    Bool,
    Void,
    Custom(TypeId),
    Unknown,
}
