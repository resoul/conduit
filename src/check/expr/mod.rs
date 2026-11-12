mod basic;
mod call;
mod member;
mod module_call;

use super::*;

impl TypeChecker {
    pub fn check_expr(&mut self, expr: &Expr) -> Result<Type, TypeError> {
        match &expr.kind {
            ExprKind::Int(_) => Ok(Type::Int),
            ExprKind::Float(_) => Ok(Type::Float),
            ExprKind::String(value) => self.check_string_expr(value, expr.span),
            ExprKind::Bool(_) => Ok(Type::Bool),
            ExprKind::Nil => Ok(Type::Nil),
            ExprKind::Ident(name) => self.check_ident_expr(name, expr.span),
            ExprKind::List(elements) => self.check_list_expr(elements, expr.span),
            ExprKind::Index { target, index } => self.check_index_expr(target, index),
            ExprKind::Closure {
                params,
                return_type,
                body,
            } => self.check_closure_expr(params, return_type.as_ref(), body, expr.span),
            ExprKind::Unary { op, expr: inner } => self.check_unary_expr(*op, inner),
            ExprKind::Binary { op, left, right } => {
                self.check_binary_expr(*op, left, right, expr.span)
            }
            ExprKind::Tuple(elements) => self.check_tuple_expr(elements),
            ExprKind::Try(inner) => self.check_try_expr(inner, expr.span),
            ExprKind::Call {
                callee,
                type_args,
                args,
                named_args,
            } => self.check_call_expr(
                callee,
                CallArgs {
                    type_args,
                    args,
                    named_args,
                },
                expr.span,
            ),
            ExprKind::Member { target, field } => {
                self.check_member_expr(target, field, expr.span)
            }
            ExprKind::OptionalMember { target, field } => {
                self.check_optional_member_expr(target, field, expr.span)
            }
        }
    }

    pub(in crate::check) fn check_expr_with_expected(
        &mut self,
        expr: &Expr,
        expected: &Type,
    ) -> Result<Type, TypeError> {
        match (&expr.kind, expected) {
            (ExprKind::List(elements), Type::List(expected_element)) => {
                for element in elements {
                    let actual = self.check_expr_with_expected(element, expected_element)?;
                    if !actual.is_assignable_to(expected_element) {
                        return Err(TypeError {
                            message: format!(
                                "List element type '{}' is not assignable to expected type '{}'",
                                actual, expected_element
                            ),
                            span: element.span,
                        });
                    }
                }
                Ok(expected.clone())
            }
            (ExprKind::List(_), Type::Optional(inner)) if matches!(**inner, Type::List(_)) => {
                self.check_expr_with_expected(expr, inner)
            }
            (ExprKind::List(_), Type::Union(variants)) => {
                let list_targets: Vec<&Type> = variants
                    .iter()
                    .filter(|variant| matches!(variant, Type::List(_)))
                    .collect();
                if let [target] = list_targets.as_slice() {
                    self.check_expr_with_expected(expr, target)
                } else {
                    self.check_expr(expr)
                }
            }
            (ExprKind::Tuple(elements), Type::Tuple(expected_elements))
                if elements.len() == expected_elements.len() =>
            {
                for (element, expected_element) in elements.iter().zip(expected_elements) {
                    let actual = self.check_expr_with_expected(element, expected_element)?;
                    if !actual.is_assignable_to(expected_element) {
                        return Err(TypeError {
                            message: format!(
                                "Tuple element type '{}' is not assignable to expected type '{}'",
                                actual, expected_element
                            ),
                            span: element.span,
                        });
                    }
                }
                Ok(expected.clone())
            }
            _ => self.check_expr(expr),
        }
    }

    pub(in crate::check) fn contains_type_param(ty: &Type) -> bool {
        match ty {
            Type::TypeParam(_) => true,
            Type::List(inner) | Type::Optional(inner) => Self::contains_type_param(inner),
            Type::Map(key, value) => {
                Self::contains_type_param(key) || Self::contains_type_param(value)
            }
            Type::Tuple(elements) | Type::Union(elements) => {
                elements.iter().any(Self::contains_type_param)
            }
            Type::Generic { type_args, .. } => type_args.iter().any(Self::contains_type_param),
            Type::Function {
                params,
                return_type,
            } => {
                params.iter().any(Self::contains_type_param)
                    || Self::contains_type_param(return_type)
            }
            _ => false,
        }
    }

    pub(super) fn classify_optional_chain(&self, ty: &Type) -> OptionalChainTarget {
        match ty {
            Type::Optional(inner) => OptionalChainTarget::Optional(inner.as_ref().clone()),
            Type::Union(variants) => {
                let has_nil = variants.contains(&Type::Nil);
                let (successes, errors) = self.split_success_and_errors(ty);
                if has_nil && errors.is_empty() {
                    let non_nil: Vec<Type> = successes
                        .into_iter()
                        .filter(|t| t != &Type::Nil)
                        .collect();
                    if non_nil.is_empty() {
                        OptionalChainTarget::Invalid
                    } else {
                        OptionalChainTarget::Optional(Type::union(non_nil))
                    }
                } else if !has_nil && !errors.is_empty() && !successes.is_empty() {
                    OptionalChainTarget::Fallible(successes, errors)
                } else {
                    OptionalChainTarget::Invalid
                }
            }
            _ => OptionalChainTarget::Invalid,
        }
    }

    pub(super) fn validate_propagated_errors(
        &self,
        errors: &[Type],
        span: Span,
    ) -> Result<(), TypeError> {
        if let Some(ref current_ret) = self.current_return_type {
            for err in errors {
                if !err.is_assignable_to(current_ret) {
                    return Err(TypeError {
                        message: format!(
                            "Error '{}' propagated by '?' is not assignable to function return type '{}'",
                            err, current_ret
                        ),
                        span,
                    });
                }
            }
            Ok(())
        } else {
            Err(TypeError {
                message: "Operator '?' cannot be used outside of a function".to_string(),
                span,
            })
        }
    }
}
