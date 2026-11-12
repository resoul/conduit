use super::*;

impl Interpreter {
    pub(super) fn is_mutable_place(expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Ident(_) => true,
            ExprKind::Member { target, .. } => Self::is_mutable_place(target),
            _ => false,
        }
    }

    pub(super) fn eval_mutating_method_on_place(
        &mut self,
        place: &Expr,
        field: &str,
        args: &[Expr],
    ) -> Result<Value, InterpFlow> {
        if field == "append" && args.len() == 1 {
            let value = self.eval_expr(&args[0])?;
            return self
                .mutate_place(place, move |slot| {
                    if let Value::List(items) = slot {
                        items.push(value);
                        Ok(Value::Void)
                    } else {
                        Err(RuntimeError {
                            message: format!("Expected List, got {}", slot.type_name()),
                            span: place.span,
                        })
                    }
                })
                .map_err(Into::into);
        }

        if field == "set" && args.len() == 2 {
            let key_val = self.eval_expr(&args[0])?;
            let key = MapKey::from_value(&key_val).ok_or_else(|| RuntimeError {
                message: format!(
                    "Map key must be String, Int, or Bool, got {}",
                    key_val.type_name()
                ),
                span: args[0].span,
            })?;
            let value = self.eval_expr(&args[1])?;
            return self
                .mutate_place(place, move |slot| {
                    if let Value::Map(map) = slot {
                        map.set(key, value);
                        Ok(Value::Void)
                    } else {
                        Err(RuntimeError {
                            message: format!("Expected Map, got {}", slot.type_name()),
                            span: place.span,
                        })
                    }
                })
                .map_err(Into::into);
        }

        if field == "remove" && args.len() == 1 {
            let key_val = self.eval_expr(&args[0])?;
            let key = MapKey::from_value(&key_val).ok_or_else(|| RuntimeError {
                message: format!(
                    "Map key must be String, Int, or Bool, got {}",
                    key_val.type_name()
                ),
                span: args[0].span,
            })?;
            return self
                .mutate_place(place, move |slot| {
                    if let Value::Map(map) = slot {
                        Ok(map.remove(&key).unwrap_or(Value::Nil))
                    } else {
                        Err(RuntimeError {
                            message: format!("Expected Map, got {}", slot.type_name()),
                            span: place.span,
                        })
                    }
                })
                .map_err(Into::into);
        }

        let target_val = self.eval_expr(place)?;
        self.eval_method_on_value(target_val, Some(place), field, args, place.span)
    }

    pub(super) fn eval_zero_arg_method_on_ref(
        target_val: &Value,
        field: &str,
        call_span: Span,
    ) -> Result<Option<Value>, InterpFlow> {
        let res = match target_val {
            Value::Int(n) => match field {
                "to_string" => Value::String(n.to_string()),
                "to_float" => Value::Float(*n as f64),
                _ => return Ok(None),
            },
            Value::Float(f) => match field {
                "floor" => checked_float_to_int(f.floor(), call_span)?,
                "ceil" => checked_float_to_int(f.ceil(), call_span)?,
                "round" => checked_float_to_int(f.round(), call_span)?,
                "trunc" => checked_float_to_int(f.trunc(), call_span)?,
                "to_string" => Value::String(f.to_string()),
                "is_nan" => Value::Bool(f.is_nan()),
                "is_infinite" => Value::Bool(f.is_infinite()),
                "is_finite" => Value::Bool(f.is_finite()),
                _ => return Ok(None),
            },
            Value::Bool(b) => match field {
                "to_string" => Value::String(b.to_string()),
                _ => return Ok(None),
            },
            Value::String(s) => match field {
                "trim" => Value::String(s.trim().to_string()),
                "lower" => Value::String(s.to_lowercase()),
                "upper" => Value::String(s.to_uppercase()),
                "to_string" => Value::String(s.clone()),
                "len" => Value::Int(s.len() as i64),
                "char_count" => Value::Int(s.chars().count() as i64),
                "to_int" => match s.parse::<i64>() {
                    Ok(n) => Value::Int(n),
                    Err(e) => make_parse_error(s, format!("Invalid Int '{}': {}", s, e)),
                },
                "to_float" => match s.parse::<f64>() {
                    Ok(f) if f.is_finite() => Value::Float(f),
                    Ok(_) => make_parse_error(
                        s,
                        format!("Invalid Float '{}': non-finite value is not allowed", s),
                    ),
                    Err(e) => make_parse_error(s, format!("Invalid Float '{}': {}", s, e)),
                },
                _ => return Ok(None),
            },
            Value::List(items) => match field {
                "len" => Value::Int(items.len() as i64),
                "reverse" => {
                    let mut rev = items.clone();
                    rev.reverse();
                    Value::List(rev)
                }
                "enumerate" => {
                    let enumerated = items
                        .iter()
                        .enumerate()
                        .map(|(i, item)| Value::Tuple(vec![Value::Int(i as i64), item.clone()]))
                        .collect();
                    Value::List(enumerated)
                }
                _ => return Ok(None),
            },
            Value::Map(map) => match field {
                "len" => Value::Int(map.len() as i64),
                "keys" => Value::List(map.keys()),
                "values" => Value::List(map.values()),
                "entries" => {
                    let list = map
                        .entries()
                        .iter()
                        .map(|(k, v)| Value::Tuple(vec![k.to_value(), v.clone()]))
                        .collect();
                    Value::List(list)
                }
                _ => return Ok(None),
            },
            _ => return Ok(None),
        };
        Ok(Some(res))
    }

    pub(super) fn eval_member_on_value(
        target_val: &Value,
        field: &str,
        span: Span,
    ) -> Result<Value, InterpFlow> {
        match target_val {
            Value::Struct { .. } | Value::Error { .. } => {
                if let Some(val) = target_val.field(field) {
                    Ok(val.clone())
                } else {
                    Err(RuntimeError {
                        message: format!("Field '{}' not found", field),
                        span,
                    }
                    .into())
                }
            }
            _ => Err(RuntimeError {
                message: format!("Field access not supported on type {}", target_val.type_name()),
                span,
            }
            .into()),
        }
    }

    pub(super) fn eval_method_on_value(
        &mut self,
        target_val: Value,
        place_expr: Option<&Expr>,
        field: &str,
        args: &[Expr],
        call_span: Span,
    ) -> Result<Value, InterpFlow> {
        if let Value::Native(ref nat) = target_val
            && let Some(res) = self.try_eval_native_method(nat, field, args)
        {
            return res;
        }

        if let Some(place) = place_expr
            && let Some(res) =
                self.try_eval_place_mutating_method(&target_val, place, field, args)
        {
            return res;
        }

        match target_val {
            Value::Int(n) => Self::eval_int_method(n, field, call_span),
            Value::Float(f) => Self::eval_float_method(f, field, call_span),
            Value::Bool(b) => {
                if field == "to_string" {
                    Ok(Value::String(b.to_string()))
                } else {
                    Err(RuntimeError {
                        message: format!("Unknown method '{}()'", field),
                        span: call_span,
                    }
                    .into())
                }
            }
            Value::String(s) => self.eval_string_method(s, field, args, call_span),
            Value::List(items) => self.eval_list_method(items, field, args, call_span),
            Value::Map(map) => self.eval_map_method(map, field, args, call_span),
            _ => Err(RuntimeError {
                message: format!("Unknown method '{}()'", field),
                span: call_span,
            }
            .into()),
        }
    }

    fn try_eval_native_method(
        &mut self,
        nat: &Arc<dyn crate::native::NativeValue>,
        field: &str,
        args: &[Expr],
    ) -> Option<Result<Value, InterpFlow>> {
        let type_id = nat.type_id();
        let method_impl = if let ModuleId::Native(ref nat_name) = type_id.module {
            self.native_registry
                .get_module(nat_name)
                .and_then(|m| m.methods.get(&(type_id.name.as_str(), field)))
                .map(|md| md.implementation)
        } else {
            None
        }?;

        let mut evaluated = Vec::new();
        for a in args {
            match self.eval_expr(a) {
                Ok(v) => evaluated.push(v),
                Err(e) => return Some(Err(e)),
            }
        }
        let res = (method_impl)(nat, evaluated);
        Some(match res {
            Ok(v) => Ok(v),
            Err(err_v) => Ok(err_v),
        })
    }

    fn try_eval_place_mutating_method(
        &mut self,
        target_val: &Value,
        place: &Expr,
        field: &str,
        args: &[Expr],
    ) -> Option<Result<Value, InterpFlow>> {
        if matches!(target_val, Value::List(_)) && field == "append" && args.len() == 1 {
            let val_to_append = match self.eval_expr(&args[0]) {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            let pspan = place.span;
            return Some(
                self.mutate_place(place, move |slot| {
                    if let Value::List(items) = slot {
                        items.push(val_to_append);
                        Ok(Value::Void)
                    } else {
                        Err(RuntimeError {
                            message: format!("Expected List, got {}", slot.type_name()),
                            span: pspan,
                        })
                    }
                })
                .map_err(Into::into),
            );
        }

        if matches!(target_val, Value::Map(_))
            && ((field == "set" && args.len() == 2) || (field == "remove" && args.len() == 1))
        {
            let key_val = match self.eval_expr(&args[0]) {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            let key = match MapKey::from_value(&key_val) {
                Some(k) => k,
                None => {
                    return Some(Err(RuntimeError {
                        message: format!(
                            "Map key must be String, Int, or Bool, got {}",
                            key_val.type_name()
                        ),
                        span: args[0].span,
                    }
                    .into()));
                }
            };
            if field == "set" {
                let val = match self.eval_expr(&args[1]) {
                    Ok(v) => v,
                    Err(e) => return Some(Err(e)),
                };
                let pspan = place.span;
                return Some(
                    self.mutate_place(place, move |slot| {
                        if let Value::Map(map) = slot {
                            map.set(key, val);
                            Ok(Value::Void)
                        } else {
                            Err(RuntimeError {
                                message: format!("Expected Map, got {}", slot.type_name()),
                                span: pspan,
                            })
                        }
                    })
                    .map_err(Into::into),
                );
            } else {
                let pspan = place.span;
                return Some(
                    self.mutate_place(place, move |slot| {
                        if let Value::Map(map) = slot {
                            Ok(map.remove(&key).unwrap_or(Value::Nil))
                        } else {
                            Err(RuntimeError {
                                message: format!("Expected Map, got {}", slot.type_name()),
                                span: pspan,
                            })
                        }
                    })
                    .map_err(Into::into),
                );
            }
        }

        None
    }

    fn eval_int_method(n: i64, field: &str, call_span: Span) -> Result<Value, InterpFlow> {
        match field {
            "to_string" => Ok(Value::String(n.to_string())),
            "to_float" => Ok(Value::Float(n as f64)),
            _ => Err(RuntimeError {
                message: format!("Unknown method '{}()'", field),
                span: call_span,
            }
            .into()),
        }
    }

    fn eval_float_method(f: f64, field: &str, call_span: Span) -> Result<Value, InterpFlow> {
        match field {
            "floor" => checked_float_to_int(f.floor(), call_span).map_err(Into::into),
            "ceil" => checked_float_to_int(f.ceil(), call_span).map_err(Into::into),
            "round" => checked_float_to_int(f.round(), call_span).map_err(Into::into),
            "trunc" => checked_float_to_int(f.trunc(), call_span).map_err(Into::into),
            "to_string" => Ok(Value::String(f.to_string())),
            "is_nan" => Ok(Value::Bool(f.is_nan())),
            "is_infinite" => Ok(Value::Bool(f.is_infinite())),
            "is_finite" => Ok(Value::Bool(f.is_finite())),
            _ => Err(RuntimeError {
                message: format!("Unknown method '{}()'", field),
                span: call_span,
            }
            .into()),
        }
    }

    fn eval_string_method(
        &mut self,
        s: String,
        field: &str,
        args: &[Expr],
        call_span: Span,
    ) -> Result<Value, InterpFlow> {
        match field {
            "trim" => return Ok(Value::String(s.trim().to_string())),
            "lower" => return Ok(Value::String(s.to_lowercase())),
            "upper" => return Ok(Value::String(s.to_uppercase())),
            "to_string" => return Ok(Value::String(s)),
            "len" => return Ok(Value::Int(s.len() as i64)),
            "char_count" => return Ok(Value::Int(s.chars().count() as i64)),
            "to_int" => {
                return match s.parse::<i64>() {
                    Ok(n) => Ok(Value::Int(n)),
                    Err(e) => Ok(make_parse_error(&s, format!("Invalid Int '{}': {}", s, e))),
                };
            }
            "to_float" => {
                return match s.parse::<f64>() {
                    Ok(f) if f.is_finite() => Ok(Value::Float(f)),
                    Ok(_) => Ok(make_parse_error(
                        &s,
                        format!("Invalid Float '{}': non-finite value is not allowed", s),
                    )),
                    Err(e) => Ok(make_parse_error(&s, format!("Invalid Float '{}': {}", s, e))),
                };
            }
            "contains" => {
                let search = self.eval_expr(&args[0])?;
                if let Value::String(sub) = search {
                    return Ok(Value::Bool(s.contains(&sub)));
                }
            }
            "index_of" => {
                let search = self.eval_expr(&args[0])?;
                if let Value::String(sub) = search {
                    return match s.find(&sub) {
                        Some(byte_idx) => {
                            let char_idx = s[..byte_idx].chars().count() as i64;
                            Ok(Value::Int(char_idx))
                        }
                        None => Ok(Value::Nil),
                    };
                }
            }
            "starts_with" => {
                let arg = self.eval_expr(&args[0])?;
                if let Value::String(prefix) = arg {
                    return Ok(Value::Bool(s.starts_with(&prefix)));
                }
            }
            "ends_with" => {
                let arg = self.eval_expr(&args[0])?;
                if let Value::String(suffix) = arg {
                    return Ok(Value::Bool(s.ends_with(&suffix)));
                }
            }
            "split" => {
                let arg = self.eval_expr(&args[0])?;
                if let Value::String(sep) = arg {
                    let parts = if sep.is_empty() {
                        s.chars().map(|c| Value::String(c.to_string())).collect()
                    } else {
                        s.split(&sep).map(|p| Value::String(p.to_string())).collect()
                    };
                    return Ok(Value::List(parts));
                }
            }
            "slice" => {
                let start_val = self.eval_expr(&args[0])?;
                let end_val = self.eval_expr(&args[1])?;
                if let (Value::Int(start_i), Value::Int(end_i)) = (start_val, end_val) {
                    let start = if start_i <= 0 { 0 } else { start_i as usize };
                    let end = if end_i <= 0 { 0 } else { end_i as usize };
                    if start >= end {
                        return Ok(Value::String(String::new()));
                    }
                    let sliced: String = s.chars().skip(start).take(end - start).collect();
                    return Ok(Value::String(sliced));
                }
            }
            "replace" => {
                let from_val = self.eval_expr(&args[0])?;
                let to_val = self.eval_expr(&args[1])?;
                if let (Value::String(from), Value::String(to)) = (from_val, to_val) {
                    let replaced = if from.is_empty() {
                        s
                    } else {
                        s.replace(&from, &to)
                    };
                    return Ok(Value::String(replaced));
                }
            }
            _ => {}
        }

        Err(RuntimeError {
            message: format!("Unknown method '{}()'", field),
            span: call_span,
        }
        .into())
    }

    fn eval_list_method(
        &mut self,
        items: Vec<Value>,
        field: &str,
        args: &[Expr],
        call_span: Span,
    ) -> Result<Value, InterpFlow> {
        match field {
            "len" => return Ok(Value::Int(items.len() as i64)),
            "reverse" => {
                let mut rev = items;
                rev.reverse();
                return Ok(Value::List(rev));
            }
            "enumerate" => {
                let enumerated = items
                    .into_iter()
                    .enumerate()
                    .map(|(i, item)| Value::Tuple(vec![Value::Int(i as i64), item]))
                    .collect();
                return Ok(Value::List(enumerated));
            }
            "slice" => {
                let start_val = self.eval_expr(&args[0])?;
                let end_val = self.eval_expr(&args[1])?;
                if let (Value::Int(start_i), Value::Int(end_i)) = (start_val, end_val) {
                    let len = items.len();
                    let s = if start_i <= 0 {
                        0
                    } else {
                        (start_i as usize).min(len)
                    };
                    let e = if end_i <= 0 {
                        0
                    } else {
                        (end_i as usize).min(len)
                    };
                    if s >= e {
                        return Ok(Value::List(Vec::new()));
                    }
                    return Ok(Value::List(items[s..e].to_vec()));
                }
            }
            "join" => {
                let sep_val = self.eval_expr(&args[0])?;
                if let Value::String(sep) = sep_val {
                    let mut parts = Vec::with_capacity(items.len());
                    for item in items {
                        if let Value::String(s) = item {
                            parts.push(s);
                        }
                    }
                    return Ok(Value::String(parts.join(&sep)));
                }
            }
            "get" => {
                let idx_val = self.eval_expr(&args[0])?;
                if let Value::Int(idx) = idx_val {
                    if idx < 0 || (idx as usize) >= items.len() {
                        return Ok(Value::Nil);
                    }
                    return Ok(items[idx as usize].clone());
                }
                return Err(RuntimeError {
                    message: format!(
                        "List method 'get' expects Int index, got {}",
                        idx_val.type_name()
                    ),
                    span: args[0].span,
                }
                .into());
            }
            "contains" => {
                let search = self.eval_expr(&args[0])?;
                return Ok(Value::Bool(items.contains(&search)));
            }
            _ => {}
        }

        Err(RuntimeError {
            message: format!("Unknown method '{}()'", field),
            span: call_span,
        }
        .into())
    }

    fn eval_map_method(
        &mut self,
        map: OrderedMap,
        field: &str,
        args: &[Expr],
        call_span: Span,
    ) -> Result<Value, InterpFlow> {
        match field {
            "len" => Ok(Value::Int(map.len() as i64)),
            "get" => {
                let key_val = self.eval_expr(&args[0])?;
                let key = MapKey::from_value(&key_val).ok_or_else(|| RuntimeError {
                    message: format!(
                        "Map key must be String, Int, or Bool, got {}",
                        key_val.type_name()
                    ),
                    span: args[0].span,
                })?;
                Ok(map.get(&key).cloned().unwrap_or(Value::Nil))
            }
            "contains_key" => {
                let key_val = self.eval_expr(&args[0])?;
                let key = MapKey::from_value(&key_val).ok_or_else(|| RuntimeError {
                    message: format!(
                        "Map key must be String, Int, or Bool, got {}",
                        key_val.type_name()
                    ),
                    span: args[0].span,
                })?;
                Ok(Value::Bool(map.contains_key(&key)))
            }
            "keys" => Ok(Value::List(map.keys())),
            "values" => Ok(Value::List(map.values())),
            "entries" => {
                let list = map
                    .entries()
                    .iter()
                    .map(|(k, v)| Value::Tuple(vec![k.to_value(), v.clone()]))
                    .collect();
                Ok(Value::List(list))
            }
            _ => Err(RuntimeError {
                message: format!("Unknown method '{}()'", field),
                span: call_span,
            }
            .into()),
        }
    }
}
