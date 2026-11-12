use super::*;

impl Interpreter {
    fn resolve_struct_fields_for_json(
        &self,
        type_id: &TypeId,
        type_args: &[Type],
        span: Span,
    ) -> Option<Vec<(String, String, Type)>> {
        let key = Self::type_id_to_mod_key(type_id)?;
        let defs = self.module_struct_fields.get(&key)?;
        let tparams = self
            .module_struct_type_params
            .get(&key)
            .cloned()
            .unwrap_or_default();
        let mut bindings = HashMap::new();
        for (tp, arg) in tparams.iter().zip(type_args.iter()) {
            bindings.insert(tp.clone(), arg.clone());
        }
        let mod_opt = &key.0;
        let mut out = Vec::with_capacity(defs.len());
        for f in defs {
            let fty = self
                .resolve_ast_type_in_mod(&f.ty, mod_opt, &tparams, span)
                .ok()?;
            let concrete = Self::substitute_runtime_type(&fty, &bindings);
            let json_name = f.json_name.clone().unwrap_or_else(|| f.name.clone());
            out.push((f.name.clone(), json_name, concrete));
        }
        Some(out)
    }

    pub(super) fn execute_json_encode(&self, val: &Value) -> Value {
        let capacity = match val {
            Value::List(items) | Value::Tuple(items) => items.len().saturating_mul(64).max(64),
            Value::Struct { .. } | Value::Map(_) => 128,
            Value::String(value) => value.len().saturating_add(2),
            _ => 64,
        };
        let mut out = String::with_capacity(capacity);
        match encode_with_struct_schemas(val, "", &mut out) {
            Ok(()) => Value::String(out),
            Err((path, msg)) => make_encode_error(path, msg),
        }
    }

    fn execute_json_decode(&self, raw: &str, type_expr: &TypeExpr, span: Span) -> Result<Value, RuntimeError> {
        let cached = self.json_decode_plans.borrow().get(&self.current_module)
            .and_then(|plans| plans.get(type_expr)).cloned();
        let plan = match cached {
            Some(plan) => plan,
            None => {
                let target_ty = self.resolve_ast_type_in_mod(type_expr, &self.current_module, &[], span)?;
                let plan = Arc::new(JsonDecodePlan::compile(&target_ty, &|tid, targs| {
                    self.resolve_struct_fields_for_json(tid, targs, span)
                }));
                self.json_decode_plans.borrow_mut().entry(self.current_module.clone())
                    .or_default().insert(type_expr.clone(), Arc::clone(&plan));
                plan
            }
        };
        match decode_json_with_plan(raw, &plan, &|tid, targs| {
            self.resolve_struct_fields_for_json(tid, targs, span)
        }) {
            Ok(v) => Ok(v),
            Err((path, msg)) => Ok(make_decode_error(path, msg)),
        }
    }
    
    pub(super) fn json_argument_ref(&self, expr: &Expr) -> Option<&Value> {
        match &expr.kind {
            ExprKind::Ident(name) => self.lookup_variable_ref(name),
            ExprKind::Member { target, field } => {
                if let ExprKind::Ident(alias) = &target.kind
                    && self.module_imports.get(&self.current_module)
                        .is_some_and(|imports| imports.contains_key(alias))
                    {
                        return None;
                    }
                match self.json_argument_ref(target)? {
                    v @ (Value::Struct { .. } | Value::Error { .. }) => v.field(field),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    pub(super) fn eval_json_encode_argument(&mut self, expr: &Expr) -> Result<Value, InterpFlow> {
        if let Some(value) = self.json_argument_ref(expr) {
            return Ok(self.execute_json_encode(value));
        }
        let value = self.eval_expr(expr)?;
        Ok(self.execute_json_encode(&value))
    }

    fn decode_json_argument(
        &self,
        value: &Value,
        type_expr: Option<&TypeExpr>,
        span: Span,
        argument_span: Span,
    ) -> Result<Value, RuntimeError> {
        let raw = match value {
            Value::String(raw) => raw,
            other => return Err(RuntimeError {
                message: format!("json.decode expects String, got {}", other.type_name()),
                span: argument_span,
            }),
        };
        let type_expr = type_expr.ok_or_else(|| RuntimeError {
            message: "json.decode requires a target type argument <T>".to_string(),
            span,
        })?;
        self.execute_json_decode(raw, type_expr, span)
    }

    pub(super) fn eval_json_decode_argument(
        &mut self,
        expr: &Expr,
        type_expr: Option<&TypeExpr>,
        span: Span,
    ) -> Result<Value, InterpFlow> {
        if let Some(value) = self.json_argument_ref(expr) {
            return self.decode_json_argument(value, type_expr, span, expr.span).map_err(Into::into);
        }
        let value = self.eval_expr(expr)?;
        self.decode_json_argument(&value, type_expr, span, expr.span).map_err(Into::into)
    }

    pub(super) fn is_json_encode_function(value: &Value) -> bool {
        matches!(value, Value::NativeFunction { module, name }
            if module == "cdt.json" && name == "encode")
    }

}
