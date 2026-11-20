use std::collections::HashMap;

use crate::frontend::ast::*;
use crate::frontend::check::{CheckedProgram, Checker, format_check_err};
use crate::frontend::check_sys::ensure_db_model_plan;
use crate::frontend::lexer::Span;
use crate::frontend::monomorph_reg::query_return_types;
use crate::frontend::types::{ModuleId, Type, TypeId};

pub(crate) fn check_and_desugar_queries(
    program: &mut Program,
    checked: &mut CheckedProgram,
    all_errors: &mut Vec<String>,
) {
    let db_conn_ty = Type::Custom(TypeId::native("cdt.db", "Connection"));

    for item in &mut program.items {
        let source_file = item.source_file.clone();
        let item_span = item.span;
        let ItemKind::Query(q) = &mut item.kind else {
            continue;
        };
        let QuerySpec {
            name,
            params,
            return_type,
            model_name,
            model_span,
            select_fields,
            where_pred,
            order_by,
            limit,
            offset,
            mode,
        } = &mut **q;

        let mut local_errors: Vec<String> = Vec::new();
        let fid = checked.func_by_name.get(name.as_str()).copied();

        if params.is_empty() {
            local_errors.push(format_check_err(
                item_span,
                source_file.as_deref(),
                format!(
                    "Query '{}' must take 'conn: db.Connection' as its first parameter",
                    name
                ),
            ));
        } else if let Some(fid) = fid
            && checked.funcs[fid as usize].params.first() != Some(&db_conn_ty)
        {
            local_errors.push(format_check_err(
                params[0].span,
                source_file.as_deref(),
                format!(
                    "Query '{}' first parameter '{}' must have type db.Connection, got {}",
                    name, params[0].name, checked.funcs[fid as usize].params[0]
                ),
            ));
        }

        let sid_opt = checked.struct_by_name.get(model_name.as_str()).copied();
        let Some(sid) = sid_opt else {
            local_errors.push(format_check_err(
                *model_span,
                source_file.as_deref(),
                format!("Query '{}' references unknown model struct '{}'", name, model_name),
            ));
            all_errors.extend(local_errors);
            continue;
        };

        if let Err(e) = ensure_db_model_plan(checked, model_name) {
            local_errors.push(format_check_err(*model_span, source_file.as_deref(), e));
        }

        let (base_ret, full_ret) = query_return_types(model_name, *mode);
        if let Some(fid) = fid {
            let actual_ret = &checked.funcs[fid as usize].return_type;
            if *actual_ret != base_ret && *actual_ret != full_ret {
                local_errors.push(format_check_err(
                    item_span,
                    source_file.as_deref(),
                    format!(
                        "Query '{}' return type mismatch: expected {} (or {}), got {}",
                        name, base_ret, full_ret, actual_ret
                    ),
                ));
            }
        }

        let struct_fields: Vec<(String, Type)> = checked.structs[sid]
            .fields
            .iter()
            .map(|f| (f.name.clone(), f.ty.clone()))
            .collect();

        for (fname, fspan) in select_fields.iter() {
            if !struct_fields.iter().any(|(n, _)| n == fname) {
                local_errors.push(format_check_err(
                    *fspan,
                    source_file.as_deref(),
                    format!(
                        "Query '{}': unknown field '{}' on struct '{}'",
                        name, fname, model_name
                    ),
                ));
            }
        }

        for ord in order_by.iter() {
            if !struct_fields.iter().any(|(n, _)| n == &ord.field) {
                local_errors.push(format_check_err(
                    ord.field_span,
                    source_file.as_deref(),
                    format!(
                        "Query '{}': unknown field '{}' in order_by for struct '{}'",
                        name, ord.field, model_name
                    ),
                ));
            }
        }

        let mut tc = Checker {
            checked,
            scopes: vec![HashMap::new()],
            current_return: full_ret,
            loop_depth: 0,
            in_defer: false,
            errors: Vec::new(),
            source_file: source_file.clone(),
        };
        if let Some(fid) = fid {
            let p_tys = tc.checked.funcs[fid as usize].params.clone();
            for (p, pty) in params.iter().zip(p_tys) {
                tc.bind(p.name.clone(), pty);
            }
        }

        if let Some(pred) = where_pred {
            check_query_pred(&mut tc, name, model_name, &struct_fields, pred, &mut local_errors);
        }
        if let Some(lim) = limit {
            if let Err(e) = tc.check_expr(lim, Some(&Type::Int)) {
                local_errors.push(format_check_err(lim.span, source_file.as_deref(), e));
            } else if lim.ty != Type::Int {
                local_errors.push(format_check_err(
                    lim.span,
                    source_file.as_deref(),
                    format!("Query '{}' limit must be Int, got {}", name, lim.ty),
                ));
            }
        }
        if let Some(off) = offset {
            if let Err(e) = tc.check_expr(off, Some(&Type::Int)) {
                local_errors.push(format_check_err(off.span, source_file.as_deref(), e));
            } else if off.ty != Type::Int {
                local_errors.push(format_check_err(
                    off.span,
                    source_file.as_deref(),
                    format!("Query '{}' offset must be Int, got {}", name, off.ty),
                ));
            }
        }
        local_errors.extend(tc.errors);

        if !local_errors.is_empty() {
            all_errors.extend(local_errors);
            continue;
        }

        let conn_name = params[0].name.clone();
        let body = desugar_query_body(
            &conn_name,
            model_name,
            select_fields,
            where_pred.take(),
            order_by,
            limit.take(),
            offset.take(),
            *mode,
            item_span,
        );
        item.kind = ItemKind::Func {
            name: std::mem::take(name),
            type_params: Vec::new(),
            params: std::mem::take(params),
            return_type: return_type.take(),
            body,
            is_validator: false,
        };
    }
}

fn check_query_pred(
    tc: &mut Checker,
    query_name: &str,
    model_name: &str,
    struct_fields: &[(String, Type)],
    pred: &mut QueryPredicate,
    errors: &mut Vec<String>,
) {
    match pred {
        QueryPredicate::And(l, r) | QueryPredicate::Or(l, r) => {
            check_query_pred(tc, query_name, model_name, struct_fields, l, errors);
            check_query_pred(tc, query_name, model_name, struct_fields, r, errors);
        }
        QueryPredicate::Cmp {
            field,
            field_span,
            op,
            value,
        } => {
            let Some((_, field_ty)) = struct_fields.iter().find(|(n, _)| n == field) else {
                errors.push(format_check_err(
                    *field_span,
                    tc.source_file.as_deref(),
                    format!(
                        "Query '{}': unknown field '{}' on struct '{}'",
                        query_name, field, model_name
                    ),
                ));
                return;
            };
            let inner_ty = match field_ty {
                Type::Optional(inner) => (**inner).clone(),
                other => other.clone(),
            };
            match op {
                QueryWhereOp::In => {
                    let expected = Type::List(Box::new(inner_ty.clone()));
                    if let Err(e) = tc.check_expr(value, Some(&expected)) {
                        errors.push(format_check_err(value.span, tc.source_file.as_deref(), e));
                    } else if value.ty != expected {
                        errors.push(format_check_err(
                            value.span,
                            tc.source_file.as_deref(),
                            format!(
                                "Query '{}': 'in' for field '{}' expects {}, got {}",
                                query_name, field, expected, value.ty
                            ),
                        ));
                    }
                }
                QueryWhereOp::Like | QueryWhereOp::ILike => {
                    if inner_ty != Type::String {
                        errors.push(format_check_err(
                            *field_span,
                            tc.source_file.as_deref(),
                            format!(
                                "Query '{}': 'like'/'ilike' requires String field, '{}' is {}",
                                query_name, field, field_ty
                            ),
                        ));
                    }
                    if let Err(e) = tc.check_expr(value, Some(&Type::String)) {
                        errors.push(format_check_err(value.span, tc.source_file.as_deref(), e));
                    } else if value.ty != Type::String {
                        errors.push(format_check_err(
                            value.span,
                            tc.source_file.as_deref(),
                            format!(
                                "Query '{}': 'like'/'ilike' value must be String, got {}",
                                query_name, value.ty
                            ),
                        ));
                    }
                }
                QueryWhereOp::Eq | QueryWhereOp::Ne => {
                    if matches!(value.kind, ExprKind::Nil) {
                        let _ = tc.check_expr(value, Some(field_ty));
                    } else if let Err(e) = tc.check_expr(value, Some(&inner_ty)) {
                        errors.push(format_check_err(value.span, tc.source_file.as_deref(), e));
                    } else if value.ty != inner_ty && value.ty != *field_ty {
                        errors.push(format_check_err(
                            value.span,
                            tc.source_file.as_deref(),
                            format!(
                                "Query '{}': field '{}' has type {}, cannot compare with {}",
                                query_name, field, field_ty, value.ty
                            ),
                        ));
                    }
                }
                QueryWhereOp::Gt | QueryWhereOp::Ge | QueryWhereOp::Lt | QueryWhereOp::Le => {
                    if let Err(e) = tc.check_expr(value, Some(&inner_ty)) {
                        errors.push(format_check_err(value.span, tc.source_file.as_deref(), e));
                    } else if value.ty != inner_ty {
                        errors.push(format_check_err(
                            value.span,
                            tc.source_file.as_deref(),
                            format!(
                                "Query '{}': field '{}' has type {}, cannot compare with {}",
                                query_name, field, field_ty, value.ty
                            ),
                        ));
                    }
                }
            }
        }
    }
}

fn make_from_expr(conn_name: &str, model_name: &str, span: Span) -> Expr {
    Expr::new(
        ExprKind::Call {
            callee: Box::new(Expr::new(
                ExprKind::Member {
                    target: Box::new(Expr::new(ExprKind::Ident(conn_name.to_string()), span)),
                    field: "from".to_string(),
                },
                span,
            )),
            type_args: vec![TypeExpr::Named(model_name.to_string())],
            args: Vec::new(),
            named_args: Vec::new(),
        },
        span,
    )
}

fn make_method_call(target: Expr, method: &str, args: Vec<Expr>, span: Span) -> Expr {
    Expr::new(
        ExprKind::Call {
            callee: Box::new(Expr::new(
                ExprKind::Member {
                    target: Box::new(target),
                    field: method.to_string(),
                },
                span,
            )),
            type_args: Vec::new(),
            args,
            named_args: Vec::new(),
        },
        span,
    )
}

fn desugar_pred_expr(
    conn_name: &str,
    model_name: &str,
    base_q: Expr,
    pred: QueryPredicate,
    span: Span,
) -> Expr {
    match pred {
        QueryPredicate::Cmp {
            field,
            field_span,
            op,
            value,
        } => {
            let col_arg = Expr::new(ExprKind::String(field), field_span);
            let value = *value;
            match op {
                QueryWhereOp::Eq => make_method_call(base_q, "where_eq", vec![col_arg, value], span),
                QueryWhereOp::In => make_method_call(base_q, "where_in", vec![col_arg, value], span),
                _ => {
                    let op_str = match op {
                        QueryWhereOp::Ne => "!=",
                        QueryWhereOp::Gt => ">",
                        QueryWhereOp::Ge => ">=",
                        QueryWhereOp::Lt => "<",
                        QueryWhereOp::Le => "<=",
                        QueryWhereOp::Like => "LIKE",
                        QueryWhereOp::ILike => "ILIKE",
                        QueryWhereOp::Eq | QueryWhereOp::In => "=",
                    };
                    let op_arg = Expr::new(ExprKind::String(op_str.to_string()), span);
                    make_method_call(base_q, "where", vec![col_arg, op_arg, value], span)
                }
            }
        }
        QueryPredicate::And(l, r) => {
            let q1 = match *l {
                QueryPredicate::Or(_, _) => {
                    let sub = desugar_pred_expr(
                        conn_name,
                        model_name,
                        make_from_expr(conn_name, model_name, span),
                        *l,
                        span,
                    );
                    make_method_call(base_q, "and_query", vec![sub], span)
                }
                other => desugar_pred_expr(conn_name, model_name, base_q, other, span),
            };
            match *r {
                QueryPredicate::Or(_, _) => {
                    let sub = desugar_pred_expr(
                        conn_name,
                        model_name,
                        make_from_expr(conn_name, model_name, span),
                        *r,
                        span,
                    );
                    make_method_call(q1, "and_query", vec![sub], span)
                }
                other => desugar_pred_expr(conn_name, model_name, q1, other, span),
            }
        }
        QueryPredicate::Or(l, r) => {
            let sub_l = desugar_pred_expr(
                conn_name,
                model_name,
                make_from_expr(conn_name, model_name, span),
                *l,
                span,
            );
            let sub_r = desugar_pred_expr(
                conn_name,
                model_name,
                make_from_expr(conn_name, model_name, span),
                *r,
                span,
            );
            let combined = make_method_call(sub_l, "or_query", vec![sub_r], span);
            make_method_call(base_q, "and_query", vec![combined], span)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn desugar_query_body(
    conn_name: &str,
    model_name: &str,
    select_fields: &[(String, Span)],
    where_pred: Option<QueryPredicate>,
    order_by: &[QueryOrderItem],
    limit: Option<Expr>,
    offset: Option<Expr>,
    mode: QueryModeAst,
    span: Span,
) -> Block {
    let mut q = make_from_expr(conn_name, model_name, span);
    if !select_fields.is_empty() {
        let cols = select_fields
            .iter()
            .map(|(f, s)| Expr::new(ExprKind::String(f.clone()), *s))
            .collect();
        q = make_method_call(q, "select", vec![Expr::new(ExprKind::List(cols), span)], span);
    }
    if let Some(pred) = where_pred {
        q = desugar_pred_expr(conn_name, model_name, q, pred, span);
    }
    for ord in order_by {
        let col_arg = Expr::new(ExprKind::String(ord.field.clone()), ord.field_span);
        let dir_arg = Expr::new(
            ExprKind::String(if ord.desc { "desc" } else { "asc" }.to_string()),
            ord.field_span,
        );
        q = make_method_call(q, "order_by", vec![col_arg, dir_arg], span);
    }
    if let Some(lim) = limit {
        q = make_method_call(q, "limit", vec![lim], span);
    }
    if let Some(off) = offset {
        q = make_method_call(q, "offset", vec![off], span);
    }
    let term = match mode {
        QueryModeAst::All => "all",
        QueryModeAst::First => "first",
        QueryModeAst::Count => "count",
        QueryModeAst::Exists => "exists",
        QueryModeAst::Delete => "delete",
    };
    let final_expr = make_method_call(q, term, Vec::new(), span);
    let _ = ModuleId::Program;
    Block {
        stmts: vec![Stmt {
            kind: StmtKind::Return(Some(final_expr)),
            span,
        }],
        span,
    }
}
