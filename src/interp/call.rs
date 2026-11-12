use super::*;

impl Interpreter {
    pub fn call_module_function(
        &mut self,
        mod_path: Option<PathBuf>,
        name: &str,
        args: Vec<Value>,
        span: Span,
    ) -> Result<Value, RuntimeError> {
        if self.call_depth == 0 && !self.large_stack_active {
            self.large_stack_active = true;
            let result = std::thread::scope(|scope| {
                let handle = std::thread::Builder::new()
                    .stack_size(INTERPRETER_STACK_SIZE)
                    .spawn_scoped(scope, || {
                        self.call_module_function(mod_path, name, args, span)
                    })
                    .map_err(|error| RuntimeError {
                        message: format!("Unable to start interpreter thread: {}", error),
                        span,
                    })?;
                match handle.join() {
                    Ok(result) => result,
                    Err(panic) => std::panic::resume_unwind(panic),
                }
            });
            self.large_stack_active = false;
            return result;
        }

        if self.call_depth >= MAX_CALL_DEPTH {
            return Err(RuntimeError {
                message: "Maximum call stack depth (256) exceeded".to_string(),
                span,
            });
        }
        let key = (mod_path.clone(), name.to_string());
        let fn_def = if let Some(sig) = self.module_functions.get(&key) {
            Arc::clone(sig)
        } else if let Some(sig) = self.module_functions.get(&(None, name.to_string())) {
            Arc::clone(sig)
        } else {
            return Err(RuntimeError {
                message: format!("Function '{}' not found", name),
                span,
            });
        };
        let (params, body) = (&fn_def.0, &fn_def.1);

        let prev_mod = self.current_module.clone();
        let prev_captured = self.captured_env.take();
        let prev_env = std::mem::replace(&mut self.env, Vec::with_capacity(4));
        self.current_module = mod_path;
        self.call_depth += 1;

        self.enter_scope();
        for (i, param) in params.iter().enumerate() {
            let arg_val = args.get(i).cloned().unwrap_or(Value::Nil);
            self.define_variable(param.name.clone(), arg_val);
        }

        let mut return_val = Value::Void;
        let mut eval_err = None;
        for stmt in &body.stmts {
            match self.eval_stmt(stmt) {
                Ok(ControlFlow::Next | ControlFlow::Break | ControlFlow::Continue) => {}
                Ok(ControlFlow::Return(val)) => {
                    return_val = val;
                    break;
                }
                Err(e) => {
                    eval_err = Some(e);
                    break;
                }
            }
        }

        self.exit_scope();
        self.env = prev_env;
        self.captured_env = prev_captured;
        self.current_module = prev_mod;
        self.call_depth -= 1;
        match eval_err {
            Some(e) => Err(e),
            None => Ok(return_val),
        }
    }

    pub(super) fn call_callable(&mut self, callee_val: &Value, args: Vec<Value>, span: Span) -> Result<Value, RuntimeError> {
        match callee_val {
            Value::Function(fn_id) => {
                if let Some((mod_str, fn_name)) = fn_id.split_once("::") {
                    let mod_path = PathBuf::from(mod_str);
                    self.call_module_function(Some(mod_path), fn_name, args, span)
                } else {
                    let cur = self.current_module.clone();
                    self.call_module_function(cur, fn_id, args, span)
                }
            }
            Value::Closure(closure) => {
                if self.call_depth >= MAX_CALL_DEPTH {
                    return Err(RuntimeError {
                        message: "Maximum call stack depth (256) exceeded".to_string(),
                        span,
                    });
                }
                self.call_depth += 1;
                let prev_mod = self.current_module.clone();
                let prev_captured = self.captured_env.replace(Arc::clone(&closure.env));
                let prev_env = std::mem::replace(&mut self.env, Vec::with_capacity(4));
                self.current_module = closure.module.clone();

                self.enter_scope();
                for (i, param) in closure.params.iter().enumerate() {
                    let arg_val = args.get(i).cloned().unwrap_or(Value::Nil);
                    self.define_variable(param.name.clone(), arg_val);
                }

                let mut return_val = Value::Void;
                let mut eval_err = None;
                for stmt in &closure.body.stmts {
                    match self.eval_stmt(stmt) {
                        Ok(ControlFlow::Next | ControlFlow::Break | ControlFlow::Continue) => {}
                        Ok(ControlFlow::Return(val)) => {
                            return_val = val;
                            break;
                        }
                        Err(e) => {
                            eval_err = Some(e);
                            break;
                        }
                    }
                }

                self.exit_scope();
                self.env = prev_env;
                self.captured_env = prev_captured;
                self.current_module = prev_mod;
                self.call_depth -= 1;
                match eval_err {
                    Some(e) => Err(e),
                    None => Ok(return_val),
                }
            }
            Value::NativeFunction { module, name } => {
                if module == "cdt.validation" && name == "check" {
                    let target_val = args.into_iter().next().unwrap_or(Value::Nil);
                    return self.execute_validation_check(target_val, span);
                }
                if module == "cdt.json" && name == "encode" {
                    let target_val = args.into_iter().next().unwrap_or(Value::Nil);
                    return Ok(self.execute_json_encode(&target_val));
                }
                let m = self.native_registry.get_module(module).ok_or_else(|| RuntimeError {
                    message: format!("Unknown native module '{}'", module),
                    span,
                })?;
                let f = m.functions.get(name.as_str()).ok_or_else(|| RuntimeError {
                    message: format!("Unknown native function '{}.{}'", module, name),
                    span,
                })?;
                match (f.implementation)(args) {
                    Ok(val) => Ok(val),
                    Err(err_val) => Ok(err_val),
                }
            }
            _ => Err(RuntimeError {
                message: format!("Value '{}' is not a callable function", callee_val),
                span,
            }),
        }
    }

}
