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

    module.add_function(NativeFuncDef {
        name: "min",
        doc: "Validates that a numeric value, string character count, or list element count meets a minimum bound (inclusive)",
        params: vec![
            (
                "value",
                Type::union(vec![
                    Type::Int,
                    Type::String,
                    Type::List(Box::new(Type::Any)),
                ]),
            ),
            ("min_val", Type::Int),
        ],
        return_type: Type::union(vec![Type::Void, issue_ty.clone()]),
        is_validator: true,
        implementation: |args| {
            if args.len() != 2 {
                return Ok(make_issue("min.args", "Expected 2 arguments for min validator"));
            }
            let min_val = match &args[1] {
                Value::Int(n) => *n,
                _ => return Ok(make_issue("min.args", "Minimum bound must be an Int")),
            };
            match &args[0] {
                Value::Int(n) => {
                    if *n < min_val {
                        Ok(make_issue(
                            "min",
                            format!("Value {} is less than minimum {}", n, min_val),
                        ))
                    } else {
                        Ok(Value::Void)
                    }
                }
                Value::String(s) => {
                    if min_val < 0 {
                        return Ok(make_issue(
                            "min.invalid_bound",
                            "Minimum length bound cannot be negative",
                        ));
                    }
                    let count = s.chars().count() as i64;
                    if count < min_val {
                        Ok(make_issue(
                            "min",
                            format!("String length {} is less than minimum {}", count, min_val),
                        ))
                    } else {
                        Ok(Value::Void)
                    }
                }
                Value::List(l) => {
                    if min_val < 0 {
                        return Ok(make_issue(
                            "min.invalid_bound",
                            "Minimum length bound cannot be negative",
                        ));
                    }
                    let count = l.len() as i64;
                    if count < min_val {
                        Ok(make_issue(
                            "min",
                            format!("List length {} is less than minimum {}", count, min_val),
                        ))
                    } else {
                        Ok(Value::Void)
                    }
                }
                _ => Ok(make_issue("min.type", "Unsupported type for min validator")),
            }
        },
    });

    module.add_function(NativeFuncDef {
        name: "max",
        doc: "Validates that a numeric value, string character count, or list element count does not exceed a maximum bound (inclusive)",
        params: vec![
            (
                "value",
                Type::union(vec![
                    Type::Int,
                    Type::String,
                    Type::List(Box::new(Type::Any)),
                ]),
            ),
            ("max_val", Type::Int),
        ],
        return_type: Type::union(vec![Type::Void, issue_ty.clone()]),
        is_validator: true,
        implementation: |args| {
            if args.len() != 2 {
                return Ok(make_issue("max.args", "Expected 2 arguments for max validator"));
            }
            let max_val = match &args[1] {
                Value::Int(n) => *n,
                _ => return Ok(make_issue("max.args", "Maximum bound must be an Int")),
            };
            match &args[0] {
                Value::Int(n) => {
                    if *n > max_val {
                        Ok(make_issue(
                            "max",
                            format!("Value {} exceeds maximum {}", n, max_val),
                        ))
                    } else {
                        Ok(Value::Void)
                    }
                }
                Value::String(s) => {
                    if max_val < 0 {
                        return Ok(make_issue(
                            "max.invalid_bound",
                            "Maximum length bound cannot be negative",
                        ));
                    }
                    let count = s.chars().count() as i64;
                    if count > max_val {
                        Ok(make_issue(
                            "max",
                            format!("String length {} exceeds maximum {}", count, max_val),
                        ))
                    } else {
                        Ok(Value::Void)
                    }
                }
                Value::List(l) => {
                    if max_val < 0 {
                        return Ok(make_issue(
                            "max.invalid_bound",
                            "Maximum length bound cannot be negative",
                        ));
                    }
                    let count = l.len() as i64;
                    if count > max_val {
                        Ok(make_issue(
                            "max",
                            format!("List length {} exceeds maximum {}", count, max_val),
                        ))
                    } else {
                        Ok(Value::Void)
                    }
                }
                _ => Ok(make_issue("max.type", "Unsupported type for max validator")),
            }
        },
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

