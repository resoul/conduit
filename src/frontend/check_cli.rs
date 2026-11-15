use std::collections::HashSet;

use crate::frontend::ast::{Expr, ExprKind, NamedArg, TypeExpr, UnaryOp};
use crate::frontend::check::{CheckedProgram, Checker};
use crate::frontend::check_native::check_fields_ctor;
use crate::frontend::types::{ModuleId, Type, TypeId};
use crate::native::cli::{
    CliDefault, CliFieldPlan, CliKind, CliNodePlan, CliPlan, CliValKind,
};

impl Checker<'_> {
    pub(crate) fn check_cli_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        let cli_err = Type::Custom(TypeId::native("cdt.cli", "CliError"));
        match sym {
            "CliError" => check_fields_ctor(
                self,
                "CliError",
                &[("kind", Type::String), ("message", Type::String)],
                args,
                named_args,
                cli_err,
            ),
            "parse" | "help" => {
                if type_args.len() != 1 || !named_args.is_empty() || !args.is_empty() {
                    return Err(format!(
                        "cli.{}<T>() expects 1 type argument and 0 value arguments",
                        sym
                    ));
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                let Type::Custom(tid) = &target_ty else {
                    return Err(format!("cli.{}<T>() requires a struct type, got {}", sym, target_ty));
                };
                if tid.module != ModuleId::Program {
                    return Err(format!(
                        "cli.{}<T>() requires a program struct, got {}",
                        sym, target_ty
                    ));
                }
                ensure_cli_plan(self.checked, &tid.name)?;
                if sym == "help" {
                    Ok(Type::String)
                } else {
                    Ok(Type::union(vec![target_ty, cli_err]))
                }
            }
            "parse_args" => {
                if type_args.len() != 1 || !named_args.is_empty() || args.len() != 1 {
                    return Err(
                        "cli.parse_args<T>(args) expects 1 type argument and 1 [String] argument"
                            .to_string(),
                    );
                }
                let target_ty = self.checked.resolve_type_mut(&type_args[0])?;
                let Type::Custom(tid) = &target_ty else {
                    return Err(format!(
                        "cli.parse_args<T>() requires a struct type, got {}",
                        target_ty
                    ));
                };
                if tid.module != ModuleId::Program {
                    return Err(format!(
                        "cli.parse_args<T>() requires a program struct, got {}",
                        target_ty
                    ));
                }
                let expected_args = Type::List(Box::new(Type::String));
                self.check_expr(&mut args[0], Some(&expected_args))?;
                if args[0].ty != expected_args {
                    return Err(format!(
                        "cli.parse_args expects [String], got {}",
                        args[0].ty
                    ));
                }
                ensure_cli_plan(self.checked, &tid.name)?;
                Ok(Type::union(vec![target_ty, cli_err]))
            }
            other => Err(format!("Unknown cdt.cli symbol '{}'", other)),
        }
    }
}

fn ensure_cli_plan(checked: &mut CheckedProgram, root_name: &str) -> Result<(), String> {
    if checked.cli_plans.iter().any(|(n, _)| n == root_name) {
        return Ok(());
    }
    let mut nodes = Vec::new();
    let mut visiting = HashSet::new();
    compile_cli_node(checked, root_name, &mut nodes, &mut visiting)?;
    checked
        .cli_plans
        .push((root_name.to_string(), CliPlan { nodes: nodes.into_boxed_slice() }));
    Ok(())
}

fn compile_cli_node(
    checked: &CheckedProgram,
    struct_name: &str,
    nodes: &mut Vec<CliNodePlan>,
    visiting: &mut HashSet<String>,
) -> Result<u16, String> {
    if !visiting.insert(struct_name.to_string()) {
        return Err(format!("Cyclic CLI subcommand reference involving '{}'", struct_name));
    }
    let node_idx = nodes.len() as u16;
    nodes.push(CliNodePlan {
        name: struct_name.to_string(),
        about: String::new(),
        schema_id: 0,
        scalar_len: 0,
        obj_len: 0,
        fields: Box::new([]),
    });

    let sid = *checked
        .struct_by_name
        .get(struct_name)
        .ok_or_else(|| format!("Unknown struct '{}' in cdt.cli", struct_name))?;
    let st = &checked.structs[sid];
    let mut field_plans = Vec::with_capacity(st.fields.len());

    for f in &st.fields {
        let (is_optional, inner_ty) = match &f.ty {
            Type::Optional(inner) => (true, (**inner).clone()),
            other => (false, other.clone()),
        };

        let ann_info = f.cli_ann.as_ref().map(|a| {
            let tag = a.target.strip_prefix("cli.").unwrap_or(a.target.as_str());
            (tag, &a.args)
        });
        let kind = match ann_info {
            Some(("flag", _)) => CliKind::Flag,
            Some(("opt", _)) => CliKind::Opt,
            Some(("arg", _)) => CliKind::Arg,
            Some(("rest", _)) => CliKind::Rest,
            Some(("cmd", _)) => CliKind::Cmd,
            Some(("group", _)) => CliKind::Group,
            _ => match &inner_ty {
                Type::Bool => CliKind::Flag,
                Type::List(el) if **el == Type::String => CliKind::Rest,
                _ => CliKind::Opt,
            },
        };

        let mut short: Option<char> = None;
        let mut help = String::new();
        let mut explicit_name: Option<String> = None;

        if let Some((tag, args)) = ann_info {
            let mut strs = Vec::with_capacity(args.len());
            for arg in args {
                let ExprKind::String(s) = &arg.kind else {
                    return Err(format!(
                        "@cli.{} on '{}.{}' expects String literal arguments",
                        tag, struct_name, f.name
                    ));
                };
                strs.push(s.clone());
            }
            match tag {
                "flag" | "opt" => match strs.as_slice() {
                    [] => {}
                    [s] => {
                        let norm = s.strip_prefix('-').unwrap_or(s);
                        if norm.chars().count() == 1 {
                            short = norm.chars().next();
                        } else {
                            help = s.clone();
                        }
                    }
                    [s, h] => {
                        let norm = s.strip_prefix('-').unwrap_or(s);
                        if !norm.is_empty() {
                            if norm.chars().count() != 1 {
                                return Err(format!(
                                    "@cli.{} short flag '{}' on '{}.{}' must be 1 character",
                                    tag, s, struct_name, f.name
                                ));
                            }
                            short = norm.chars().next();
                        }
                        help = h.clone();
                    }
                    _ => {
                        return Err(format!(
                            "@cli.{} on '{}.{}' expects at most 2 String arguments",
                            tag, struct_name, f.name
                        ));
                    }
                },
                "arg" | "rest" => match strs.as_slice() {
                    [] => {}
                    [h] => help = h.clone(),
                    _ => {
                        return Err(format!(
                            "@cli.{} on '{}.{}' expects at most 1 String argument",
                            tag, struct_name, f.name
                        ));
                    }
                },
                "cmd" | "group" => match strs.as_slice() {
                    [] => {}
                    [h] => help = h.clone(),
                    [n, h] => {
                        if !n.is_empty() {
                            explicit_name = Some(n.clone());
                        }
                        help = h.clone();
                    }
                    _ => {
                        return Err(format!(
                            "@cli.{} on '{}.{}' expects at most 2 String arguments",
                            tag, struct_name, f.name
                        ));
                    }
                },
                _ => {}
            }
        }

        let (val_kind, sub_node) = match kind {
            CliKind::Flag => {
                if inner_ty != Type::Bool {
                    return Err(format!(
                        "cli: flag '{}.{}' must have type Bool or Bool?, got {}",
                        struct_name, f.name, f.ty
                    ));
                }
                (CliValKind::Bool, None)
            }
            CliKind::Rest => {
                if is_optional || inner_ty != Type::List(Box::new(Type::String)) {
                    return Err(format!(
                        "cli: rest '{}.{}' must have type [String], got {}",
                        struct_name, f.name, f.ty
                    ));
                }
                (CliValKind::ListStr, None)
            }
            CliKind::Cmd | CliKind::Group => {
                if !is_optional {
                    return Err(format!(
                        "cli: subcommand '{}.{}' must be optional (e.g. Sub?)",
                        struct_name, f.name
                    ));
                }
                let Type::Custom(sub_tid) = &inner_ty else {
                    return Err(format!(
                        "cli: subcommand '{}.{}' must reference a struct type, got {}",
                        struct_name, f.name, f.ty
                    ));
                };
                if sub_tid.module != ModuleId::Program {
                    return Err(format!(
                        "cli: subcommand '{}.{}' must reference a program struct",
                        struct_name, f.name
                    ));
                }
                let idx = compile_cli_node(checked, &sub_tid.name, nodes, visiting)?;
                (CliValKind::SubStruct, Some(idx))
            }
            CliKind::Opt | CliKind::Arg => {
                let vk = match inner_ty {
                    Type::String => CliValKind::String,
                    Type::Int => CliValKind::Int,
                    Type::Float => CliValKind::Float,
                    Type::Bool => CliValKind::Bool,
                    _ => {
                        return Err(format!(
                            "cli: field '{}.{}' has unsupported type {} (expected String, Int, Float, Bool, or optional)",
                            struct_name, f.name, f.ty
                        ));
                    }
                };
                (vk, None)
            }
        };

        let default = match &f.default {
            Some(expr) => Some(expr_to_cli_default(expr).ok_or_else(|| {
                format!(
                    "cli: default value for '{}.{}' must be a literal constant",
                    struct_name, f.name
                )
            })?),
            None => None,
        };

        let cli_name = match kind {
            CliKind::Cmd | CliKind::Group => {
                explicit_name.unwrap_or_else(|| f.name.replace('_', "-"))
            }
            CliKind::Flag | CliKind::Opt => f.name.replace('_', "-"),
            CliKind::Arg | CliKind::Rest => f.name.clone(),
        };

        field_plans.push(CliFieldPlan {
            field_name: f.name.clone(),
            cli_name,
            short,
            help,
            kind,
            val_kind,
            is_optional,
            is_scalar: f.is_scalar,
            slot_idx: f.slot_idx,
            default,
            sub_node,
        });
    }

    visiting.remove(struct_name);
    nodes[node_idx as usize] = CliNodePlan {
        name: struct_name.to_string(),
        about: checked.struct_abouts.get(struct_name).cloned().unwrap_or_default(),
        schema_id: st.schema_id,
        scalar_len: st.scalar_len,
        obj_len: st.obj_len,
        fields: field_plans.into_boxed_slice(),
    };
    Ok(node_idx)
}

fn expr_to_cli_default(expr: &Expr) -> Option<CliDefault> {
    match &expr.kind {
        ExprKind::Int(v) => Some(CliDefault::Int(*v)),
        ExprKind::Float(v) => Some(CliDefault::Float(v.to_bits())),
        ExprKind::Bool(b) => Some(CliDefault::Bool(*b)),
        ExprKind::String(s) => Some(CliDefault::Str(s.clone())),
        ExprKind::Nil => Some(CliDefault::Nil),
        ExprKind::List(items) if items.is_empty() => Some(CliDefault::EmptyList),
        ExprKind::Unary { op: UnaryOp::Neg, expr: inner } => match &inner.kind {
            ExprKind::Int(v) => Some(CliDefault::Int(-*v)),
            ExprKind::Float(v) => Some(CliDefault::Float((-*v).to_bits())),
            _ => None,
        },
        _ => None,
    }
}
