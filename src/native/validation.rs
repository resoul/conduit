use std::collections::HashMap;

use super::{NativeFuncDef, NativeModule, NativeTypeDef};
use crate::types::{Type, TypeId};
use crate::value::Value;

pub fn make_issue(code: impl Into<String>, message: impl Into<String>) -> Value {
    let mut fields = HashMap::new();
    fields.insert("code".to_string(), Value::String(code.into()));
    fields.insert("message".to_string(), Value::String(message.into()));
    Value::Error {
        type_id: TypeId::native("cdt.validation", "Issue"),
        fields,
    }
}

pub fn make_violation(path: impl Into<String>, code: impl Into<String>, message: impl Into<String>) -> Value {
    let mut fields = HashMap::new();
    fields.insert("path".to_string(), Value::String(path.into()));
    fields.insert("code".to_string(), Value::String(code.into()));
    fields.insert("message".to_string(), Value::String(message.into()));
    Value::Struct {
        type_id: TypeId::native("cdt.validation", "Violation"),
        fields,
    }
}

pub fn make_validation_error(violations: Vec<Value>) -> Value {
    let mut fields = HashMap::new();
    fields.insert("issues".to_string(), Value::List(violations));
    Value::Error {
        type_id: TypeId::native("cdt.validation", "ValidationError"),
        fields,
    }
}

pub fn validate_email_str(s: &str) -> bool {
    if s.is_empty() || s.contains(char::is_whitespace) {
        return false;
    }
    let parts: Vec<&str> = s.split('@').collect();
    if parts.len() != 2 {
        return false;
    }
    let local = parts[0];
    let domain = parts[1];
    if local.is_empty() || domain.is_empty() {
        return false;
    }
    if !domain.contains('.') {
        return false;
    }
    if domain.starts_with('.') || domain.ends_with('.') || domain.contains("..") {
        return false;
    }
    if local.starts_with('.') || local.ends_with('.') || local.contains("..") {
        return false;
    }
    true
}

fn check_bound(args: Vec<Value>, is_min: bool) -> Result<Value, Value> {
    let (code, bound_word, cmp_phrase) = if is_min {
        ("min", "Minimum", "is less than minimum")
    } else {
        ("max", "Maximum", "exceeds maximum")
    };
    if args.len() != 2 {
        return Ok(make_issue(
            format!("{}.args", code),
            format!("Expected 2 arguments for {} validator", code),
        ));
    }
    let len_check = |label: &str, count: usize, bound: i64| -> Value {
        if bound < 0 {
            return make_issue(
                format!("{}.invalid_bound", code),
                format!("{} length bound cannot be negative", bound_word),
            );
        }
        let n = count as i64;
        if (is_min && n < bound) || (!is_min && n > bound) {
            make_issue(code, format!("{} length {} {} {}", label, n, cmp_phrase, bound))
        } else {
            Value::Void
        }
    };
    match (&args[0], &args[1]) {
        (Value::Int(n), Value::Int(bound)) => {
            if (is_min && n < bound) || (!is_min && n > bound) {
                Ok(make_issue(code, format!("Value {} {} {}", n, cmp_phrase, bound)))
            } else {
                Ok(Value::Void)
            }
        }
        (Value::Float(val), Value::Float(bound)) => {
            if !val.is_finite() || !bound.is_finite() {
                return Ok(make_issue(
                    format!("{}.non_finite", code),
                    format!("{} validator requires finite Float value and bound", bound_word),
                ));
            }
            if (is_min && val < bound) || (!is_min && val > bound) {
                Ok(make_issue(code, format!("Value {} {} {}", val, cmp_phrase, bound)))
            } else {
                Ok(Value::Void)
            }
        }
        (Value::String(s), Value::Int(bound)) => Ok(len_check("String", s.chars().count(), *bound)),
        (Value::List(l), Value::Int(bound)) => Ok(len_check("List", l.len(), *bound)),
        (Value::Map(m), Value::Int(bound)) => Ok(len_check("Map", m.len(), *bound)),
        (Value::Float(_), _) => Ok(make_issue(
            format!("{}.args", code),
            format!("{} bound for Float must be a Float", bound_word),
        )),
        (Value::Int(_) | Value::String(_) | Value::List(_) | Value::Map(_), _) => Ok(make_issue(
            format!("{}.args", code),
            format!("{} bound must be an Int", bound_word),
        )),
        _ => Ok(make_issue(
            format!("{}.type", code),
            format!("Unsupported type for {} validator", code),
        )),
    }
}

pub fn create_validation_module() -> NativeModule {
    let mut module = NativeModule::new(
        "cdt.validation",
        "validation",
        "Standard library for schema validation and field rules",
    );

    let issue_ty = Type::Custom(TypeId::native("cdt.validation", "Issue"));
    let violation_ty = Type::Custom(TypeId::native("cdt.validation", "Violation"));
    let validation_error_ty = Type::Custom(TypeId::native("cdt.validation", "ValidationError"));

    module.add_type(NativeTypeDef {
        name: "Issue",
        qualified_name: "validation.Issue",
        doc: "Single validation rule violation without field path",
        is_error: true,
        fields: vec![("code", Type::String), ("message", Type::String)],
    });

    module.add_type(NativeTypeDef {
        name: "Violation",
        qualified_name: "validation.Violation",
        doc: "Field-attributed validation violation with complete path",
        is_error: false,
        fields: vec![
            ("path", Type::String),
            ("code", Type::String),
            ("message", Type::String),
        ],
    });

    module.add_type(NativeTypeDef {
        name: "ValidationError",
        qualified_name: "validation.ValidationError",
        doc: "Aggregated validation error collecting all field violations",
        is_error: true,
        fields: vec![("issues", Type::List(Box::new(violation_ty)))],
    });

    let bound_value_ty = Type::union(vec![
        Type::Int,
        Type::Float,
        Type::String,
        Type::List(Box::new(Type::Any)),
        Type::Map(Box::new(Type::Any), Box::new(Type::Any)),
    ]);
    let bound_arg_ty = Type::union(vec![Type::Int, Type::Float]);

    module.add_function(NativeFuncDef {
        name: "min",
        doc: "Validates that a numeric value, string character count, list length, or map size meets a minimum bound (inclusive)",
        params: vec![("value", bound_value_ty.clone()), ("min_val", bound_arg_ty.clone())],
        return_type: Type::union(vec![Type::Void, issue_ty.clone()]),
        is_validator: true,
        implementation: |args| check_bound(args, true),
    });

    module.add_function(NativeFuncDef {
        name: "max",
        doc: "Validates that a numeric value, string character count, list length, or map size does not exceed a maximum bound (inclusive)",
        params: vec![("value", bound_value_ty), ("max_val", bound_arg_ty)],
        return_type: Type::union(vec![Type::Void, issue_ty.clone()]),
        is_validator: true,
        implementation: |args| check_bound(args, false),
    });

    module.add_function(NativeFuncDef {
        name: "email",
        doc: "Validates that a string conforms to basic email address format (local@domain.tld)",
        params: vec![("value", Type::String)],
        return_type: Type::union(vec![Type::Void, issue_ty]),
        is_validator: true,
        implementation: |args| {
            let val = match args.into_iter().next() {
                Some(Value::String(s)) => s,
                _ => return Ok(make_issue("email.type", "Expected String argument for email validator")),
            };
            if !validate_email_str(&val) {
                Ok(make_issue("email", "Invalid email address format"))
            } else {
                Ok(Value::Void)
            }
        },
    });

    module.add_function(NativeFuncDef {
        name: "check",
        doc: "Validates a value and all its nested fields against declared annotations, returning the value or an aggregated ValidationError",
        params: vec![("value", Type::Any)],
        return_type: Type::union(vec![Type::Any, validation_error_ty]),
        is_validator: false,
        implementation: |_args| {
            Ok(Value::Void)
        },
    });

    module
}

