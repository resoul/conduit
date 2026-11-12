use super::*;

impl Interpreter {
    pub(super) fn enter_scope(&mut self) {
        self.env.push(HashMap::new());
    }

    pub(super) fn exit_scope(&mut self) {
        self.env.pop();
    }

    pub(super) fn define_variable(&mut self, name: String, val: Value) {
        if let Some(scope) = self.env.last_mut() {
            scope.insert(name, val);
        }
    }

    pub(super) fn assign_variable(&mut self, name: &str, val: Value, span: Span) -> Result<(), RuntimeError> {
        for scope in self.env.iter_mut().rev() {
            if let Some(slot) = scope.get_mut(name) {
                *slot = val;
                return Ok(());
            }
        }
        Err(RuntimeError {
            message: format!("Cannot assign to undefined variable '{}'", name),
            span,
        })
    }

    pub(super) fn lookup_variable(&self, name: &str) -> Option<Value> {
        self.lookup_variable_ref(name).cloned()
    }

    pub(super) fn lookup_variable_ref(&self, name: &str) -> Option<&Value> {
        self.env
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .or_else(|| self.captured_env.as_deref().and_then(|env| env.get(name)))
    }

    fn lookup_variable_mut(&mut self, name: &str) -> Option<&mut Value> {
        for scope in self.env.iter_mut().rev() {
            if let Some(val) = scope.get_mut(name) {
                return Some(val);
            }
        }
        None
    }

    pub(super) fn mutate_place<F>(
        &mut self,
        place: &Expr,
        mutate_fn: F,
    ) -> Result<Value, RuntimeError>
    where
        F: FnOnce(&mut Value) -> Result<Value, RuntimeError>,
    {
        let mut fields = Vec::new();
        let mut cur = place;
        let root_name = loop {
            match &cur.kind {
                ExprKind::Member { target, field } => {
                    fields.push((field.clone(), cur.span));
                    cur = target;
                }
                ExprKind::Ident(name) => break name.clone(),
                _ => {
                    return Err(RuntimeError {
                        message: "Invalid assignment target".to_string(),
                        span: place.span,
                    });
                }
            }
        };
        fields.reverse();

        let mut slot = self.lookup_variable_mut(&root_name).ok_or_else(|| RuntimeError {
            message: format!("Cannot assign to undefined variable '{}'", root_name),
            span: cur.span,
        })?;

        for (field_name, fspan) in fields {
            if matches!(slot, Value::Struct { .. }) {
                slot = slot.field_mut(&field_name).ok_or_else(|| RuntimeError {
                    message: format!("Field '{}' not found", field_name),
                    span: fspan,
                })?;
            } else {
                return Err(RuntimeError {
                    message: format!("Field access not supported on type {}", slot.type_name()),
                    span: fspan,
                });
            }
        }

        mutate_fn(slot)
    }

    pub(super) fn bind_for_pattern(&mut self, pattern: &Pattern, val: Value) {
        match &pattern.kind {
            PatternKind::Ident(var_name) => {
                self.define_variable(var_name.clone(), val);
            }
            PatternKind::Wildcard => {}
            PatternKind::Tuple(sub_pats) => {
                if let Value::Tuple(items) = val {
                    for (sub_pat, item) in sub_pats.iter().zip(items) {
                        self.bind_for_pattern(sub_pat, item);
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn snapshot_env(&self) -> Arc<HashMap<String, Value>> {
        let mut captured = match &self.captured_env {
            Some(env) => (**env).clone(),
            None => HashMap::new(),
        };
        for scope in &self.env {
            for (k, v) in scope {
                captured.insert(k.clone(), v.clone());
            }
        }
        Arc::new(captured)
    }

    fn push_unescaped_braces(dst: &mut String, text: &str) {
        for ch in text.chars() {
            if ch == ESCAPED_OPEN_BRACE {
                dst.push('{');
            } else if ch == ESCAPED_CLOSE_BRACE {
                dst.push('}');
            } else {
                dst.push(ch);
            }
        }
    }

    pub(super) fn interpolate(&self, template: &str) -> String {
        let mut result = String::new();
        let mut chars = template.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == ESCAPED_OPEN_BRACE {
                result.push('{');
                continue;
            }
            if ch == ESCAPED_CLOSE_BRACE {
                result.push('}');
                continue;
            }
            if ch != '{' {
                result.push(ch);
                continue;
            }

            let mut segment = String::new();
            let mut closed = false;
            for inner in chars.by_ref() {
                if inner == '}' {
                    closed = true;
                    break;
                }
                segment.push(inner);
            }
            if closed {
                let parts: Vec<&str> = segment.split('.').collect();
                if !parts.is_empty() && parts.iter().all(|p| Self::is_simple_identifier(p))
                    && let Some(mut current) = self.lookup_variable(parts[0]) {
                        let mut resolved = true;
                        for part in &parts[1..] {
                            match current {
                                Value::Struct { .. } | Value::Error { .. } => {
                                    if let Some(val) = current.field(part) {
                                        current = val.clone();
                                    } else {
                                        resolved = false;
                                        break;
                                    }
                                }
                                _ => {
                                    resolved = false;
                                    break;
                                }
                            }
                        }
                        if resolved {
                            result.push_str(&current.to_string());
                            continue;
                        }
                    }
            }
            result.push('{');
            Self::push_unescaped_braces(&mut result, &segment);
            if closed {
                result.push('}');
            }
        }

        result
    }

    fn is_simple_identifier(value: &str) -> bool {
        let mut bytes = value.bytes();
        matches!(bytes.next(), Some(b'a'..=b'z' | b'A'..=b'Z' | b'_'))
            && bytes.all(|b| matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_'))
    }

}
