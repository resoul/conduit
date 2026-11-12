use super::super::*;

impl Interpreter {
    #[inline]
    pub(super) fn eval_call_expr(
        &mut self,
        callee: &Expr,
        type_args: &[TypeExpr],
        args: &[Expr],
        named_args: &[NamedArg],
        span: Span,
    ) -> Result<Value, InterpFlow> {
        if let ExprKind::Ident(name) = &callee.kind {
            if let Some(res) =
                self.try_eval_local_var_call(name, callee.span, type_args, args, named_args)
            {
                return res;
            }
            if let Some(res) =
                self.try_eval_generic_module_call(name, callee.span, type_args, args, named_args)
            {
                return res;
            }
            if let Some(res) =
                self.try_eval_named_call(name, type_args, args, named_args, span)
            {
                return res;
            }
        }

        if let ExprKind::Member { target, field } = &callee.kind {
            return self.eval_member_call_expr(
                target,
                field,
                callee.span,
                CallArgs {
                    type_args,
                    args,
                    named_args,
                },
                span,
            );
        }

        if let ExprKind::OptionalMember { target, field } = &callee.kind {
            return self.eval_optional_member_call_expr(target, field, args, span);
        }

        let callee_val = self.eval_expr(callee)?;
        if Self::is_json_encode_function(&callee_val) && args.len() == 1 {
            return self.eval_json_encode_argument(&args[0]);
        }
        let mut evaluated_args = Vec::new();
        for a in args {
            evaluated_args.push(self.eval_expr(a)?);
        }
        self.call_callable(&callee_val, evaluated_args, callee.span)
            .map_err(Into::into)
    }

    #[inline]
    fn is_ident_constructor_or_builtin(&self, name: &str) -> (bool, bool) {
        let is_constructor = name == "Map"
            || self
                .module_structs
                .get(&self.current_module)
                .is_some_and(|types| types.contains(name))
            || self
                .module_errors
                .get(&self.current_module)
                .is_some_and(|errors| errors.contains(name))
            || matches!(name, "IndexError" | "KeyError" | "ParseError");
        let is_builtin = matches!(name, "map" | "filter" | "print");
        (is_constructor, is_builtin)
    }

    #[inline]
    fn try_eval_local_var_call(
        &mut self,
        name: &str,
        callee_span: Span,
        type_args: &[TypeExpr],
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Option<Result<Value, InterpFlow>> {
        let (is_constructor, is_builtin) = self.is_ident_constructor_or_builtin(name);
        if is_constructor || is_builtin || !type_args.is_empty() || !named_args.is_empty() {
            return None;
        }
        let callee_val = self.lookup_variable(name)?;
        if Self::is_json_encode_function(&callee_val) && args.len() == 1 {
            return Some(self.eval_json_encode_argument(&args[0]));
        }
        let mut evaluated_args = Vec::with_capacity(args.len());
        for arg in args {
            match self.eval_expr(arg) {
                Ok(v) => evaluated_args.push(v),
                Err(e) => return Some(Err(e)),
            }
        }
        Some(
            self.call_callable(&callee_val, evaluated_args, callee_span)
                .map_err(Into::into),
        )
    }

    fn try_eval_generic_module_call(
        &mut self,
        name: &str,
        callee_span: Span,
        type_args: &[TypeExpr],
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Option<Result<Value, InterpFlow>> {
        let (is_constructor, is_builtin) = self.is_ident_constructor_or_builtin(name);
        let module_function_exists = self
            .module_functions
            .contains_key(&(self.current_module.clone(), name.to_string()))
            || self
                .module_functions
                .contains_key(&(None, name.to_string()));
        if is_constructor
            || is_builtin
            || type_args.is_empty()
            || !named_args.is_empty()
            || !module_function_exists
        {
            return None;
        }
        let mut evaluated_args = Vec::with_capacity(args.len());
        for arg in args {
            match self.eval_expr(arg) {
                Ok(v) => evaluated_args.push(v),
                Err(e) => return Some(Err(e)),
            }
        }
        Some(
            self.call_module_function(
                self.current_module.clone(),
                name,
                evaluated_args,
                callee_span,
            )
            .map_err(Into::into),
        )
    }

    fn try_eval_named_call(
        &mut self,
        name: &str,
        type_args: &[TypeExpr],
        args: &[Expr],
        named_args: &[NamedArg],
        span: Span,
    ) -> Option<Result<Value, InterpFlow>> {
        let cur_structs = self.module_structs.get(&self.current_module);
        let is_struct = cur_structs.map(|s| s.contains(name)).unwrap_or(false);
        let cur_errors = self.module_errors.get(&self.current_module);
        let is_error = cur_errors.map(|e| e.contains(name)).unwrap_or(false);

        if is_struct || is_error || matches!(name, "IndexError" | "KeyError" | "ParseError") {
            let type_id = if is_struct || is_error {
                TypeId::from_module_opt(self.current_module.as_ref(), name.to_string())
            } else {
                TypeId::builtin(name)
            };
            return Some(self.construct_record(type_id, !is_struct, named_args));
        }
        if name == "Map" && args.is_empty() && named_args.is_empty() {
            return Some(Ok(Value::Map(OrderedMap::new())));
        }

        if name == "map" && args.len() == 2 {
            let list_val = match self.eval_expr(&args[0]) {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            let cb_val = match self.eval_expr(&args[1]) {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            if let Value::List(items) = list_val {
                let mut results = Vec::new();
                for item in items {
                    if item.is_error() {
                        results.push(item);
                    } else {
                        match self.call_callable(&cb_val, vec![item], args[1].span) {
                            Ok(res) => results.push(res),
                            Err(e) => return Some(Err(e.into())),
                        }
                    }
                }
                return Some(Ok(Value::List(results)));
            }
        }

        if name == "filter" && args.len() == 2 {
            let list_val = match self.eval_expr(&args[0]) {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            let cb_val = match self.eval_expr(&args[1]) {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            if let Value::List(items) = list_val {
                let mut results = Vec::new();
                for item in items {
                    if item.is_error() {
                        results.push(item);
                    } else {
                        match self.call_callable(&cb_val, vec![item.clone()], args[1].span) {
                            Ok(res) => {
                                if res.is_truthy() {
                                    results.push(item);
                                }
                            }
                            Err(e) => return Some(Err(e.into())),
                        }
                    }
                }
                return Some(Ok(Value::List(results)));
            }
        }

        if name == "print" {
            if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
                return Some(Err(RuntimeError {
                    message: format!(
                        "Function 'print' expects exactly 1 argument, got {}",
                        args.len()
                    ),
                    span,
                }
                .into()));
            }
            let val = match self.eval_expr(&args[0]) {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            print!("{}", val);
            println!();
            return Some(Ok(Value::Void));
        }

        None
    }

    #[inline]
    fn eval_member_call_expr(
        &mut self,
        target: &Expr,
        field: &str,
        callee_span: Span,
        call: CallArgs<'_>,
        span: Span,
    ) -> Result<Value, InterpFlow> {
        let CallArgs {
            type_args,
            args,
            named_args,
        } = call;
        if let ExprKind::Ident(ref alias) = target.kind {
            if alias == "Map" && field == "from" && args.len() == 1 {
                return self.eval_map_from_call(&args[0]);
            }
            let cur_imports = self.module_imports.get(&self.current_module).cloned();
            if let Some(target_mod) = cur_imports.and_then(|m| m.get(alias).cloned()) {
                match target_mod {
                    ModuleTarget::Native(nat_name) => {
                        if let Some(res) = self.try_eval_native_module_call(
                            &nat_name,
                            field,
                            callee_span,
                            type_args,
                            args,
                            named_args,
                        ) {
                            return res;
                        }
                    }
                    ModuleTarget::Local(path) => {
                        return self.eval_local_module_call(
                            path,
                            field,
                            callee_span,
                            args,
                            named_args,
                        );
                    }
                }
            }
        }

        if Self::is_mutable_place(target)
            && ((field == "append" && args.len() == 1)
                || (field == "set" && args.len() == 2)
                || (field == "remove" && args.len() == 1))
        {
            return self.eval_mutating_method_on_place(target, field, args);
        }

        if args.is_empty()
            && let Some(target_ref) = self.json_argument_ref(target)
            && let Some(res) = Self::eval_zero_arg_method_on_ref(target_ref, field, span)?
        {
            return Ok(res);
        }

        let target_val = self.eval_expr(target)?;
        self.eval_method_on_value(target_val, Some(target), field, args, span)
    }

    fn eval_map_from_call(&mut self, arg: &Expr) -> Result<Value, InterpFlow> {
        let pairs_val = self.eval_expr(arg)?;
        if let Value::List(items) = pairs_val {
            let mut map = OrderedMap::new();
            for item in items {
                if let Value::Tuple(elems) = item
                    && elems.len() == 2
                    && let Some(key) = MapKey::from_value(&elems[0])
                {
                    map.set(key, elems[1].clone());
                    continue;
                }
                return Err(RuntimeError {
                    message: "Map.from expects a list of (key, value) pairs with String, Int, or Bool keys".to_string(),
                    span: arg.span,
                }
                .into());
            }
            return Ok(Value::Map(map));
        }
        Err(RuntimeError {
            message: format!("Map.from expects a List, got {}", pairs_val.type_name()),
            span: arg.span,
        }
        .into())
    }

    fn try_eval_native_module_call(
        &mut self,
        nat_name: &str,
        field: &str,
        callee_span: Span,
        type_args: &[TypeExpr],
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Option<Result<Value, InterpFlow>> {
        if nat_name == "cdt.validation" && field == "check" {
            let target_val = if !args.is_empty() {
                match self.eval_expr(&args[0]) {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                }
            } else {
                Value::Nil
            };
            return Some(
                self.execute_validation_check(target_val, callee_span)
                    .map_err(InterpFlow::Error),
            );
        }

        if nat_name == "cdt.json" && field == "decode" {
            return Some(self.eval_json_decode_argument(&args[0], type_args.first(), callee_span));
        }

        if nat_name == "cdt.json" && field == "encode" {
            return Some(self.eval_json_encode_argument(&args[0]));
        }

        let (is_constructible, is_error) = {
            let m = match self.native_registry.get_module(nat_name) {
                Some(m) => m,
                None => {
                    return Some(Err(RuntimeError {
                        message: format!("Unknown native module '{}'", nat_name),
                        span: callee_span,
                    }
                    .into()));
                }
            };
            if let Some(td) = m.types.get(field) {
                (!td.fields.is_empty(), td.is_error)
            } else {
                (false, false)
            }
        };

        if is_constructible {
            return Some(self.construct_record(
                TypeId::native(nat_name.to_string(), field.to_string()),
                is_error,
                named_args,
            ));
        }

        let func_impl = {
            let m = match self.native_registry.get_module(nat_name) {
                Some(m) => m,
                None => {
                    return Some(Err(RuntimeError {
                        message: format!("Unknown native module '{}'", nat_name),
                        span: callee_span,
                    }
                    .into()));
                }
            };
            m.functions.get(field).map(|f| f.implementation)
        };
        if let Some(impl_fn) = func_impl {
            let mut evaluated = Vec::new();
            for a in args {
                match self.eval_expr(a) {
                    Ok(v) => evaluated.push(v),
                    Err(e) => return Some(Err(e)),
                }
            }
            let res = (impl_fn)(evaluated);
            return Some(match res {
                Ok(val) => Ok(val),
                Err(err_val) => Ok(err_val),
            });
        }

        None
    }

    fn eval_local_module_call(
        &mut self,
        path: PathBuf,
        field: &str,
        callee_span: Span,
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Result<Value, InterpFlow> {
        let is_struct = self
            .module_structs
            .get(&Some(path.clone()))
            .map(|s| s.contains(field))
            .unwrap_or(false);
        let is_error = self
            .module_errors
            .get(&Some(path.clone()))
            .map(|e| e.contains(field))
            .unwrap_or(false);

        if is_struct || is_error {
            return self.construct_record(
                TypeId::local(path, field.to_string()),
                is_error,
                named_args,
            );
        }

        let mut evaluated = Vec::new();
        for a in args {
            evaluated.push(self.eval_expr(a)?);
        }
        self.call_module_function(Some(path), field, evaluated, callee_span)
            .map_err(Into::into)
    }

    fn eval_optional_member_call_expr(
        &mut self,
        target: &Expr,
        field: &str,
        args: &[Expr],
        span: Span,
    ) -> Result<Value, InterpFlow> {
        let target_val = self.eval_expr(target)?;
        if target_val == Value::Nil {
            return Ok(Value::Nil);
        }
        if target_val.is_error() {
            return Err(InterpFlow::Return(target_val));
        }
        self.eval_method_on_value(target_val, None, field, args, span)
    }
}

