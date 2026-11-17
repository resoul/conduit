use std::collections::{HashMap, HashSet};

use crate::frontend::ast::*;
pub use crate::frontend::check_native::{resolve_type_expr, type_to_json_type};
use crate::frontend::check_native::{
    is_error_type, native_member_field, register_native_imports, validator_email_fn_ty,
};
use crate::frontend::check_validation::{
    build_validation_plans, check_validator_contracts, finalize_validation_plan_func_ids,
};
use crate::frontend::monomorph::{GenericFuncTemplate, GenericStructTemplate, resolve_switch_pattern_type};
use crate::frontend::monomorph_reg::register_program_types;
use crate::frontend::monomorph_subst::resolve_type_expr_with_scope;
use crate::frontend::reachability::shake_unreachable_funcs;
use crate::frontend::types::{FuncSig, ModuleId, ResolvedStruct, Type, TypeId};
use crate::native::validation::ValidationStructPlan;
use crate::native::StructSchema;

#[derive(Debug, Clone)]
pub enum ConstVal {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
}

#[derive(Debug)]
pub struct CheckedProgram {
    pub structs: Vec<ResolvedStruct>,
    pub struct_by_name: HashMap<String, usize>,
    pub funcs: Vec<FuncSig>,
    pub func_by_name: HashMap<String, u32>,
    pub struct_schemas: Vec<StructSchema>,
    pub json_aliases: HashSet<String>,
    pub json_direct_fns: HashMap<String, String>,
    pub json_direct_types: HashMap<String, String>,
    pub time_aliases: HashSet<String>,
    pub time_direct_fns: HashMap<String, String>,
    pub time_direct_types: HashMap<String, String>,
    pub val_aliases: HashSet<String>,
    pub val_direct_fns: HashMap<String, String>,
    pub val_direct_types: HashMap<String, String>,
    pub env_aliases: HashSet<String>,
    pub env_direct_fns: HashMap<String, String>,
    pub env_direct_types: HashMap<String, String>,
    pub env_plans: Vec<crate::native::env::EnvLoadPlan>,
    pub cli_aliases: HashSet<String>,
    pub cli_direct_fns: HashMap<String, String>,
    pub cli_direct_types: HashMap<String, String>,
    pub cli_plans: Vec<(String, crate::native::cli::CliPlan)>,
    pub fs_aliases: HashSet<String>,
    pub fs_direct_fns: HashMap<String, String>,
    pub fs_direct_types: HashMap<String, String>,
    pub path_aliases: HashSet<String>,
    pub path_direct_fns: HashMap<String, String>,
    pub process_aliases: HashSet<String>,
    pub process_direct_fns: HashMap<String, String>,
    pub process_direct_types: HashMap<String, String>,
    pub crypto_aliases: HashSet<String>,
    pub crypto_direct_fns: HashMap<String, String>,
    pub crypto_direct_types: HashMap<String, String>,
    pub url_aliases: HashSet<String>,
    pub url_direct_fns: HashMap<String, String>,
    pub url_direct_types: HashMap<String, String>,
    pub csv_aliases: HashSet<String>,
    pub csv_direct_fns: HashMap<String, String>,
    pub csv_direct_types: HashMap<String, String>,
    pub math_aliases: HashSet<String>,
    pub math_direct_fns: HashMap<String, String>,
    pub log_aliases: HashSet<String>,
    pub log_direct_fns: HashMap<String, String>,
    pub struct_abouts: HashMap<String, String>,
    pub validator_funcs: HashSet<String>,
    pub error_structs: HashSet<String>,
    pub validation_plans: Vec<ValidationStructPlan>,
    pub generic_structs: HashMap<String, GenericStructTemplate>,
    pub generic_funcs: HashMap<String, GenericFuncTemplate>,
    pub mono_struct_args: HashMap<String, (String, Vec<Type>)>,
    pub pending_mono_items: Vec<Item>,
    pub consts: HashMap<String, (Type, ConstVal)>,
}

impl CheckedProgram {
    pub fn resolve_type(&self, te: &TypeExpr) -> Result<Type, String> {
        resolve_type_expr(te, self)
    }

    pub(crate) fn resolve_type_mut(&mut self, te: &TypeExpr) -> Result<Type, String> {
        let empty = HashSet::new();
        resolve_type_expr_with_scope(te, self, &empty)
    }
}

pub fn check_program(program: &mut Program) -> Result<CheckedProgram, String> {
    check_program_with_roots(program, None)
}

pub fn check_program_with_roots(
    program: &mut Program,
    explicit_roots: Option<&[&str]>,
) -> Result<CheckedProgram, String> {
    let mut checked = CheckedProgram {
        structs: Vec::new(),
        struct_by_name: HashMap::new(),
        funcs: Vec::new(),
        func_by_name: HashMap::new(),
        struct_schemas: Vec::new(),
        json_aliases: HashSet::from(["json".to_string()]),
        json_direct_fns: HashMap::new(),
        json_direct_types: HashMap::new(),
        time_aliases: HashSet::new(),
        time_direct_fns: HashMap::new(),
        time_direct_types: HashMap::new(),
        val_aliases: HashSet::new(),
        val_direct_fns: HashMap::new(),
        val_direct_types: HashMap::new(),
        env_aliases: HashSet::new(),
        env_direct_fns: HashMap::new(),
        env_direct_types: HashMap::new(),
        env_plans: Vec::new(),
        cli_aliases: HashSet::new(),
        cli_direct_fns: HashMap::new(),
        cli_direct_types: HashMap::new(),
        cli_plans: Vec::new(),
        fs_aliases: HashSet::new(),
        fs_direct_fns: HashMap::new(),
        fs_direct_types: HashMap::new(),
        path_aliases: HashSet::new(),
        path_direct_fns: HashMap::new(),
        process_aliases: HashSet::new(),
        process_direct_fns: HashMap::new(),
        process_direct_types: HashMap::new(),
        crypto_aliases: HashSet::new(),
        crypto_direct_fns: HashMap::new(),
        crypto_direct_types: HashMap::new(),
        url_aliases: HashSet::new(),
        url_direct_fns: HashMap::new(),
        url_direct_types: HashMap::new(),
        csv_aliases: HashSet::new(),
        csv_direct_fns: HashMap::new(),
        csv_direct_types: HashMap::new(),
        math_aliases: HashSet::new(),
        math_direct_fns: HashMap::new(),
        log_aliases: HashSet::new(),
        log_direct_fns: HashMap::new(),
        struct_abouts: HashMap::new(),
        validator_funcs: HashSet::new(),
        error_structs: HashSet::new(),
        validation_plans: Vec::new(),
        generic_structs: HashMap::new(),
        generic_funcs: HashMap::new(),
        mono_struct_args: HashMap::new(),
        pending_mono_items: Vec::new(),
        consts: HashMap::new(),
    };

    register_native_imports(program, &mut checked)?;
    register_program_types(program, &mut checked)?;
    register_program_consts(program, &mut checked)?;
    check_validator_contracts(program, &mut checked)?;
    check_default_exprs(program, &mut checked)?;

    let mut idx = 0usize;
    while idx < program.items.len() {
        let is_concrete_fn = matches!(
            &program.items[idx].kind,
            ItemKind::Func { type_params, .. } if type_params.is_empty()
        );
        if is_concrete_fn {
            let ItemKind::Func {
                name,
                params,
                body,
                ..
            } = &mut program.items[idx].kind
            else {
                unreachable!();
            };
            let fid = checked.func_by_name[name];
            let sig = checked.funcs[fid as usize].clone();
            let mut tc = Checker {
                checked: &mut checked,
                scopes: vec![HashMap::new()],
                current_return: sig.return_type.clone(),
                loop_depth: 0,
            };
            for (param, ty) in params.iter().zip(sig.params.iter()) {
                tc.bind(param.name.clone(), ty.clone());
            }
            tc.check_block(body)?;
            if !checked.pending_mono_items.is_empty() {
                let extra = std::mem::take(&mut checked.pending_mono_items);
                program.items.extend(extra);
            }
        }
        idx += 1;
    }

    if !checked.pending_mono_items.is_empty() {
        let extra = std::mem::take(&mut checked.pending_mono_items);
        program.items.extend(extra);
    }
    program.items.retain(|item| match &item.kind {
        ItemKind::Struct { type_params, .. } | ItemKind::Func { type_params, .. } => {
            type_params.is_empty()
        }
        _ => true,
    });

    build_validation_plans(program, &mut checked)?;
    shake_unreachable_funcs(program, &mut checked, explicit_roots)?;
    finalize_validation_plan_func_ids(&mut checked);
    Ok(checked)
}

fn check_default_exprs(program: &mut Program, checked: &mut CheckedProgram) -> Result<(), String> {
    for item in &mut program.items {
        match &mut item.kind {
            ItemKind::Struct { name, type_params, fields, .. } if type_params.is_empty() => {
                let sid = checked.struct_by_name[name];
                for (i, f) in fields.iter_mut().enumerate() {
                    if let Some(def_expr) = &mut f.default {
                        let expected = checked.structs[sid].fields[i].ty.clone();
                        let mut tc = Checker {
                            checked,
                            scopes: vec![HashMap::new()],
                            current_return: Type::Void,
                            loop_depth: 0,
                        };
                        tc.check_expr(def_expr, Some(&expected))?;
                        if !def_expr.ty.is_assignable_to(&expected) {
                            return Err(format!(
                                "Default value for field '{}.{}' expected {}, got {}",
                                name, f.name, expected, def_expr.ty
                            ));
                        }
                        checked.structs[sid].fields[i].default = Some(def_expr.clone());
                    }
                }
            }
            ItemKind::Func { name, type_params, params, .. } if type_params.is_empty() => {
                let fid = checked.func_by_name[name] as usize;
                for (i, p) in params.iter_mut().enumerate() {
                    if let Some(def_expr) = &mut p.default {
                        let expected = checked.funcs[fid].params[i].clone();
                        let mut tc = Checker {
                            checked,
                            scopes: vec![HashMap::new()],
                            current_return: Type::Void,
                            loop_depth: 0,
                        };
                        tc.check_expr(def_expr, Some(&expected))?;
                        if !def_expr.ty.is_assignable_to(&expected) {
                            return Err(format!(
                                "Default value for parameter '{}' of '{}' expected {}, got {}",
                                p.name, name, expected, def_expr.ty
                            ));
                        }
                        checked.funcs[fid].defaults[i] = Some(def_expr.clone());
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) struct Checker<'a> {
    pub(crate) checked: &'a mut CheckedProgram,
    scopes: Vec<HashMap<String, Type>>,
    pub(crate) current_return: Type,
    pub(crate) loop_depth: usize,
}

impl<'a> Checker<'a> {
    pub(crate) fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub(crate) fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    pub(crate) fn bind(&mut self, name: String, ty: Type) {
        if let Some(top) = self.scopes.last_mut() {
            top.insert(name, ty);
        }
    }

    pub(crate) fn is_local(&self, name: &str) -> bool {
        self.scopes.iter().any(|scope| scope.contains_key(name))
    }

    pub(crate) fn lookup(&self, name: &str) -> Option<Type> {
        for scope in self.scopes.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty.clone());
            }
        }
        if let Some(&fid) = self.checked.func_by_name.get(name) {
            let sig = &self.checked.funcs[fid as usize];
            return Some(Type::Function {
                params: sig.params.clone(),
                return_type: Box::new(sig.return_type.clone()),
            });
        }
        None
    }

    fn check_block(&mut self, block: &mut Block) -> Result<(), String> {
        self.push_scope();
        for stmt in &mut block.stmts {
            self.check_stmt(stmt)?;
        }
        self.pop_scope();
        Ok(())
    }

    pub(crate) fn check_stmt(&mut self, stmt: &mut Stmt) -> Result<(), String> {
        match &mut stmt.kind {
            StmtKind::Define { name, value } => {
                self.check_expr(value, None)?;
                if value.ty == Type::Void {
                    return Err(format!("Cannot assign Void to '{}'", name));
                }
                self.bind(name.clone(), value.ty.clone());
            }
            StmtKind::DestructureDefine { pattern, value } => {
                self.check_expr(value, None)?;
                self.bind_pattern(pattern, &value.ty)?;
            }
            StmtKind::VarDecl { name, ty, init } => {
                let decl_ty = match ty {
                    Some(te) => Some(self.checked.resolve_type_mut(te)?),
                    None => None,
                };
                if let Some(init_expr) = init {
                    self.check_expr(init_expr, decl_ty.as_ref())?;
                    if let Some(expected) = &decl_ty {
                        if !init_expr.ty.is_assignable_to(expected) {
                            return Err(format!(
                                "Cannot initialize '{}' ({}): got {}",
                                name, expected, init_expr.ty
                            ));
                        }
                        self.bind(name.clone(), expected.clone());
                    } else {
                        self.bind(name.clone(), init_expr.ty.clone());
                    }
                } else if let Some(expected) = decl_ty {
                    self.bind(name.clone(), expected);
                } else {
                    return Err(format!(
                        "Variable '{}' requires a type annotation or initializer",
                        name
                    ));
                }
            }
            StmtKind::Assign { target, value } => {
                if matches!(target.kind, ExprKind::Index { .. }) {
                    return Err("Index assignment is not supported".to_string());
                }
                if let ExprKind::Ident(name) = &target.kind
                    && self.lookup(name).is_none()
                    && self.checked.consts.contains_key(name)
                {
                    return Err(format!("Cannot assign to constant '{}'", name));
                }
                self.check_expr(target, None)?;
                self.check_expr(value, Some(&target.ty))?;
                if !value.ty.is_assignable_to(&target.ty) {
                    return Err(format!(
                        "Cannot assign {} to {}",
                        value.ty, target.ty
                    ));
                }
            }
            StmtKind::Return(opt) => {
                if let Some(expr) = opt {
                    let expected_ret = self.current_return.clone();
                    self.check_expr(expr, Some(&expected_ret))?;
                    if !expr.ty.is_assignable_to(&self.current_return) {
                        return Err(format!(
                            "Return type mismatch: expected {}, got {}",
                            self.current_return, expr.ty
                        ));
                    }
                } else if !Type::Void.is_assignable_to(&self.current_return) {
                    return Err(format!(
                        "Empty return in function returning {}",
                        self.current_return
                    ));
                }
            }
            StmtKind::If {
                condition,
                then_block,
                else_block,
            } => {
                self.check_expr(condition, Some(&Type::Bool))?;
                if condition.ty != Type::Bool {
                    return Err(format!("If condition must be Bool, got {}", condition.ty));
                }
                self.check_block(then_block)?;
                if let Some(eb) = else_block {
                    self.check_block(eb)?;
                }
            }
            StmtKind::Guard {
                condition,
                else_block,
            } => {
                self.check_expr(condition, Some(&Type::Bool))?;
                if condition.ty != Type::Bool {
                    return Err(format!(
                        "Guard condition must be Bool, got {}",
                        condition.ty
                    ));
                }
                self.check_block(else_block)?;
            }
            StmtKind::ForIn {
                pattern,
                iterable,
                body,
            } => {
                self.check_expr(iterable, None)?;
                let elem_ty = match &iterable.ty {
                    Type::List(inner) => (**inner).clone(),
                    Type::Map(k, v) => Type::Tuple(vec![(**k).clone(), (**v).clone()]),
                    other => return Err(format!("Cannot iterate over {}", other)),
                };
                self.loop_depth += 1;
                self.push_scope();
                self.bind_pattern(pattern, &elem_ty)?;
                for s in &mut body.stmts {
                    self.check_stmt(s)?;
                }
                self.pop_scope();
                self.loop_depth -= 1;
            }
            StmtKind::While { condition, body } => {
                self.check_expr(condition, Some(&Type::Bool))?;
                if condition.ty != Type::Bool {
                    return Err(format!(
                        "While condition must be Bool, got {}",
                        condition.ty
                    ));
                }
                self.loop_depth += 1;
                self.check_block(body)?;
                self.loop_depth -= 1;
            }
            StmtKind::Switch { expr, cases } => {
                self.check_expr(expr, None)?;
                for case in cases {
                    self.push_scope();
                    self.bind_switch_pattern(&case.pattern, &expr.ty)?;
                    for s in &mut case.body.stmts {
                        self.check_stmt(s)?;
                    }
                    self.pop_scope();
                }
            }
            StmtKind::Break | StmtKind::Continue => {
                if self.loop_depth == 0 {
                    return Err("break/continue used outside loop".to_string());
                }
            }
            StmtKind::Expr(e) => self.check_expr(e, None)?,
        }
        Ok(())
    }

    fn bind_pattern(&mut self, pat: &Pattern, ty: &Type) -> Result<(), String> {
        match (&pat.kind, ty) {
            (PatternKind::Wildcard | PatternKind::Nil, _) => Ok(()),
            (PatternKind::Ident(name), _) => {
                self.bind(name.clone(), ty.clone());
                Ok(())
            }
            (PatternKind::Tuple(pats), Type::Tuple(tys)) if pats.len() == tys.len() => {
                for (p, t) in pats.iter().zip(tys.iter()) {
                    self.bind_pattern(p, t)?;
                }
                Ok(())
            }
            _ => Err(format!("Pattern {:?} does not match type {}", pat.kind, ty)),
        }
    }

    fn bind_switch_pattern(&mut self, pat: &Pattern, scrutinee_ty: &Type) -> Result<(), String> {
        if let PatternKind::Type { name, binding } = &pat.kind {
            let resolved = resolve_switch_pattern_type(name, scrutinee_ty, self.checked)?;
            if binding != "_" {
                self.bind(binding.clone(), resolved);
            }
            Ok(())
        } else {
            self.bind_pattern(pat, scrutinee_ty)
        }
    }

    pub(crate) fn check_expr(&mut self, expr: &mut Expr, expected: Option<&Type>) -> Result<(), String> {
        let mut const_inline: Option<ExprKind> = None;
        let ty = match &mut expr.kind {
            ExprKind::Int(_) => Type::Int,
            ExprKind::Float(_) => Type::Float,
            ExprKind::String(_) => {
                let ty = self.check_and_desugar_string_literal(expr)?;
                expr.ty = ty;
                return Ok(());
            }
            ExprKind::Bool(_) => Type::Bool,
            ExprKind::Nil => expected
                .filter(|t| matches!(t, Type::Optional(_) | Type::Union(_)))
                .cloned()
                .unwrap_or(Type::Nil),
            ExprKind::Ident(name) => {
                if let Some(ty) = self.lookup(name) {
                    ty
                } else if let Some((cty, cval)) = self.checked.consts.get(name) {
                    const_inline = Some(match cval {
                        ConstVal::Int(n) => ExprKind::Int(*n),
                        ConstVal::Float(f) => ExprKind::Float(*f),
                        ConstVal::Bool(b) => ExprKind::Bool(*b),
                        ConstVal::String(s) => ExprKind::String(s.clone()),
                    });
                    cty.clone()
                } else if self.checked.generic_funcs.contains_key(name) {
                    return Err(format!(
                        "Generic function '{}' cannot be used as a first-class value",
                        name
                    ));
                } else if self.checked.val_direct_fns.get(name).map(String::as_str) == Some("email") {
                    validator_email_fn_ty()
                } else {
                    return Err(format!("Undefined identifier '{}'", name));
                }
            }
            ExprKind::Binary { op, left, right } => self.check_binary(*op, left, right)?,
            ExprKind::Unary { op, expr: inner } => {
                self.check_expr(inner, expected)?;
                match op {
                    UnaryOp::Neg if inner.ty.is_int() || inner.ty.is_float() => inner.ty.clone(),
                    UnaryOp::Not if inner.ty.is_bool() => Type::Bool,
                    _ => return Err(format!("Invalid unary {:?} for {}", op, inner.ty)),
                }
            }
            ExprKind::List(elems) => {
                if elems.is_empty() {
                    match expected {
                        Some(Type::List(inner)) => Type::List(inner.clone()),
                        _ => Type::List(Box::new(Type::Any)),
                    }
                } else {
                    let elem_exp = match expected {
                        Some(Type::List(inner)) => Some(inner.as_ref()),
                        _ => None,
                    };
                    self.check_expr(&mut elems[0], elem_exp)?;
                    let list_elem_ty = if let Some(exp) = elem_exp
                        && *exp != Type::Any
                        && elems[0].ty.is_assignable_to(exp)
                    {
                        exp.clone()
                    } else {
                        elems[0].ty.clone()
                    };
                    for el in elems.iter_mut().skip(1) {
                        self.check_expr(el, Some(&list_elem_ty))?;
                        if !el.ty.is_assignable_to(&list_elem_ty) {
                            return Err(format!(
                                "List element type mismatch: expected {}, got {}",
                                list_elem_ty, el.ty
                            ));
                        }
                    }
                    Type::List(Box::new(list_elem_ty))
                }
            }
            ExprKind::Tuple(elems) => {
                let mut tys = Vec::with_capacity(elems.len());
                for e in elems {
                    self.check_expr(e, None)?;
                    tys.push(e.ty.clone());
                }
                Type::Tuple(tys)
            }
            ExprKind::Try(inner) => {
                self.check_expr(inner, None)?;
                match &inner.ty {
                    Type::Union(vars) => {
                        let non_err: Vec<Type> = vars
                            .iter()
                            .filter(|v| !is_error_type(v, self.checked))
                            .cloned()
                            .collect();
                        if non_err.is_empty() {
                            inner.ty.clone()
                        } else {
                            Type::union(non_err)
                        }
                    }
                    Type::Optional(t) => (**t).clone(),
                    other => other.clone(),
                }
            }
            ExprKind::Member { target, field } => {
                if let ExprKind::Ident(alias) = &target.kind
                    && self.lookup(alias).is_none()
                    && self.checked.val_aliases.contains(alias)
                    && field == "email"
                {
                    target.ty = Type::Void;
                    validator_email_fn_ty()
                } else {
                    self.check_expr(target, None)?;
                    match &target.ty {
                        Type::Custom(tid) => match &tid.module {
                            ModuleId::Program => {
                                let sid = self.checked.struct_by_name.get(&tid.name).ok_or_else(|| {
                                    format!("Unknown struct '{}' on field '.{}'", tid, field)
                                })?;
                                let st = &self.checked.structs[*sid];
                                st.field(field)
                                    .map(|f| f.ty.clone())
                                    .ok_or_else(|| format!("Type '{}' has no field '{}'", tid, field))?
                            }
                            ModuleId::Native(_) | ModuleId::Builtin => {
                                native_member_field(tid, field)?.0
                            }
                            _ => return Err(format!("Cannot access field '.{}' on {}", field, tid)),
                        },
                        other => {
                            return Err(format!("Cannot access field '.{}' on {}", field, other));
                        }
                    }
                }
            }
            ExprKind::OptionalMember { target, field } => {
                self.check_expr(target, None)?;
                let inner = match &target.ty {
                    Type::Optional(t) => (**t).clone(),
                    other => return Err(format!("Cannot use '?.' on {}", other)),
                };
                let field_ty = match &inner {
                    Type::Custom(tid) => match &tid.module {
                        ModuleId::Program => {
                            let sid = self.checked.struct_by_name.get(&tid.name).ok_or_else(|| {
                                format!("Unknown struct '{}' on optional field '.{}'", tid, field)
                            })?;
                            self.checked.structs[*sid]
                                .field(field)
                                .map(|f| f.ty.clone())
                                .ok_or_else(|| format!("Type '{}' has no field '{}'", tid, field))?
                        }
                        ModuleId::Native(_) | ModuleId::Builtin => {
                            native_member_field(tid, field)?.0
                        }
                        _ => return Err(format!("Cannot use '?.' on {}", inner)),
                    },
                    _ => return Err(format!("Unknown optional field '.{}'", field)),
                };
                if matches!(field_ty, Type::Optional(_)) {
                    field_ty
                } else {
                    Type::Optional(Box::new(field_ty))
                }
            }
            ExprKind::Index { target, index } => {
                self.check_expr(target, None)?;
                match &target.ty.clone() {
                    Type::List(inner) => {
                        self.check_expr(index, Some(&Type::Int))?;
                        if index.ty != Type::Int {
                            return Err(format!("List index must be Int, got {}", index.ty));
                        }
                        Type::union(vec![
                            (**inner).clone(),
                            Type::Custom(TypeId {
                                module: ModuleId::Builtin,
                                name: "IndexError".to_string(),
                            }),
                        ])
                    }
                    Type::Map(k, v) => {
                        self.check_expr(index, Some(k))?;
                        if !index.ty.is_assignable_to(k) {
                            return Err(format!(
                                "Map key must be {}, got {}",
                                k, index.ty
                            ));
                        }
                        Type::union(vec![
                            (**v).clone(),
                            Type::Custom(TypeId {
                                module: ModuleId::Builtin,
                                name: "KeyError".to_string(),
                            }),
                        ])
                    }
                    other => {
                        return Err(format!("Cannot index into non-indexable type '{}'", other));
                    }
                }
            }
            ExprKind::Closure {
                params,
                return_type,
                body,
            } => self.check_closure(params, return_type, body, expected)?,
            ExprKind::Call {
                callee,
                type_args,
                args,
                named_args,
            } => self.check_call(callee, type_args, args, named_args)?,
        };
        if let Some(kind) = const_inline {
            expr.kind = kind;
        }
        expr.ty = ty;
        Ok(())
    }

    pub(crate) fn check_map_from(
        &mut self,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &[NamedArg],
    ) -> Result<Type, String> {
        if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
            return Err("Map.from(pairs) expects 1 argument".to_string());
        }
        self.check_expr(&mut args[0], None)?;
        if let Type::List(inner) = &args[0].ty
            && let Type::Tuple(elems) = inner.as_ref()
            && elems.len() == 2
        {
            crate::frontend::check_native::validate_map_key_type(&elems[0])?;
            if elems[0] == Type::Any {
                return Err("Map.from() key type cannot be Any".to_string());
            }
            return Ok(Type::Map(Box::new(elems[0].clone()), Box::new(elems[1].clone())));
        }
        Err(format!("Map.from() expects [(K, V)], got {}", args[0].ty))
    }

    pub(crate) fn check_prelude_error_ctor(
        &mut self,
        name: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        if !type_args.is_empty() {
            return Err(format!("'{}' takes no type arguments", name));
        }
        let schema: &[(&str, Type)] = match name {
            "IndexError" => &[("index", Type::Int), ("length", Type::Int), ("message", Type::String)],
            "KeyError" => &[("key", Type::String), ("message", Type::String)],
            _ => &[("value", Type::String), ("message", Type::String)],
        };
        if !named_args.is_empty() {
            if !args.is_empty() || named_args.len() != schema.len() {
                return Err(format!("'{}' expects {} arguments", name, schema.len()));
            }
            let mut seen = std::collections::HashSet::new();
            for na in named_args.iter_mut() {
                if !seen.insert(na.name.clone()) {
                    return Err(format!("Duplicate field '{}' in '{}'", na.name, name));
                }
                let (_, fty) = schema
                    .iter()
                    .find(|(f, _)| *f == na.name)
                    .ok_or_else(|| format!("Type '{}' has no field '{}'", name, na.name))?;
                self.check_expr(&mut na.value, Some(fty))?;
                if !na.value.ty.is_assignable_to(fty) {
                    return Err(format!("Field '{}' expected {}, got {}", na.name, fty, na.value.ty));
                }
            }
        } else {
            if args.len() != schema.len() {
                return Err(format!("'{}' expects {} arguments", name, schema.len()));
            }
            for (arg, (fname, fty)) in args.iter_mut().zip(schema.iter()) {
                self.check_expr(arg, Some(fty))?;
                if !arg.ty.is_assignable_to(fty) {
                    return Err(format!("Field '{}' expected {}, got {}", fname, fty, arg.ty));
                }
            }
        }
        Ok(Type::Custom(TypeId { module: ModuleId::Builtin, name: name.to_string() }))
    }
}

fn eval_const_expr(
    expr: &Expr,
    consts: &HashMap<String, (Type, ConstVal)>,
) -> Result<(Type, ConstVal), String> {
    match &expr.kind {
        ExprKind::Int(n) => Ok((Type::Int, ConstVal::Int(*n))),
        ExprKind::Float(f) => Ok((Type::Float, ConstVal::Float(*f))),
        ExprKind::Bool(b) => Ok((Type::Bool, ConstVal::Bool(*b))),
        ExprKind::String(s) => {
            if s.contains('{') || s.contains("\\{") {
                return Err("Constant string cannot contain interpolation".to_string());
            }
            Ok((Type::String, ConstVal::String(s.clone())))
        }
        ExprKind::Ident(name) => consts
            .get(name)
            .cloned()
            .ok_or_else(|| format!("Unknown or non-constant identifier '{}' in const initializer", name)),
        ExprKind::Unary { op, expr: inner } => {
            let (ty, val) = eval_const_expr(inner, consts)?;
            match (op, val) {
                (UnaryOp::Neg, ConstVal::Int(n)) => n
                    .checked_neg()
                    .map(|v| (Type::Int, ConstVal::Int(v)))
                    .ok_or_else(|| "Integer overflow in const negation".to_string()),
                (UnaryOp::Neg, ConstVal::Float(f)) => Ok((Type::Float, ConstVal::Float(-f))),
                (UnaryOp::Not, ConstVal::Bool(b)) => Ok((Type::Bool, ConstVal::Bool(!b))),
                _ => Err(format!("Invalid unary {:?} in const expression for {}", op, ty)),
            }
        }
        ExprKind::Binary { op, left, right } => {
            let (_, lv) = eval_const_expr(left, consts)?;
            let (_, rv) = eval_const_expr(right, consts)?;
            match (op, lv, rv) {
                (BinaryOp::Add, ConstVal::Int(a), ConstVal::Int(b)) => a
                    .checked_add(b)
                    .map(|v| (Type::Int, ConstVal::Int(v)))
                    .ok_or_else(|| "Integer overflow in const '+'".to_string()),
                (BinaryOp::Sub, ConstVal::Int(a), ConstVal::Int(b)) => a
                    .checked_sub(b)
                    .map(|v| (Type::Int, ConstVal::Int(v)))
                    .ok_or_else(|| "Integer overflow in const '-'".to_string()),
                (BinaryOp::Mul, ConstVal::Int(a), ConstVal::Int(b)) => a
                    .checked_mul(b)
                    .map(|v| (Type::Int, ConstVal::Int(v)))
                    .ok_or_else(|| "Integer overflow in const '*'".to_string()),
                (BinaryOp::Div, ConstVal::Int(a), ConstVal::Int(b)) => {
                    if b == 0 {
                        Err("Division by zero in const '/'".to_string())
                    } else {
                        a.checked_div(b)
                            .map(|v| (Type::Int, ConstVal::Int(v)))
                            .ok_or_else(|| "Integer overflow in const '/'".to_string())
                    }
                }
                (BinaryOp::Mod, ConstVal::Int(a), ConstVal::Int(b)) => {
                    if b == 0 {
                        Err("Division by zero in const '%'".to_string())
                    } else {
                        a.checked_rem(b)
                            .map(|v| (Type::Int, ConstVal::Int(v)))
                            .ok_or_else(|| "Integer overflow in const '%'".to_string())
                    }
                }
                (BinaryOp::Add, ConstVal::Float(a), ConstVal::Float(b)) => {
                    Ok((Type::Float, ConstVal::Float(a + b)))
                }
                (BinaryOp::Sub, ConstVal::Float(a), ConstVal::Float(b)) => {
                    Ok((Type::Float, ConstVal::Float(a - b)))
                }
                (BinaryOp::Mul, ConstVal::Float(a), ConstVal::Float(b)) => {
                    Ok((Type::Float, ConstVal::Float(a * b)))
                }
                (BinaryOp::Div, ConstVal::Float(a), ConstVal::Float(b)) => {
                    Ok((Type::Float, ConstVal::Float(a / b)))
                }
                (BinaryOp::Add, ConstVal::String(a), ConstVal::String(b)) => {
                    Ok((Type::String, ConstVal::String(format!("{}{}", a, b))))
                }
                (BinaryOp::Eq, ConstVal::Int(a), ConstVal::Int(b)) => Ok((Type::Bool, ConstVal::Bool(a == b))),
                (BinaryOp::NotEq, ConstVal::Int(a), ConstVal::Int(b)) => Ok((Type::Bool, ConstVal::Bool(a != b))),
                (BinaryOp::Lt, ConstVal::Int(a), ConstVal::Int(b)) => Ok((Type::Bool, ConstVal::Bool(a < b))),
                (BinaryOp::LtEq, ConstVal::Int(a), ConstVal::Int(b)) => Ok((Type::Bool, ConstVal::Bool(a <= b))),
                (BinaryOp::Gt, ConstVal::Int(a), ConstVal::Int(b)) => Ok((Type::Bool, ConstVal::Bool(a > b))),
                (BinaryOp::GtEq, ConstVal::Int(a), ConstVal::Int(b)) => Ok((Type::Bool, ConstVal::Bool(a >= b))),
                (BinaryOp::And, ConstVal::Bool(a), ConstVal::Bool(b)) => Ok((Type::Bool, ConstVal::Bool(a && b))),
                (BinaryOp::Or, ConstVal::Bool(a), ConstVal::Bool(b)) => Ok((Type::Bool, ConstVal::Bool(a || b))),
                _ => Err("Unsupported binary expression in const initializer".to_string()),
            }
        }
        _ => Err("Const initializer must be a compile-time constant (Int, Float, Bool, String, or arithmetic on constants)".to_string()),
    }
}

fn register_program_consts(
    program: &Program,
    checked: &mut CheckedProgram,
) -> Result<(), String> {
    for item in &program.items {
        if let ItemKind::Const { name, ty, value } = &item.kind {
            if checked.consts.contains_key(name)
                || checked.func_by_name.contains_key(name)
                || checked.struct_by_name.contains_key(name)
            {
                return Err(format!("Duplicate top-level symbol '{}'", name));
            }
            let (val_ty, val) = eval_const_expr(value, &checked.consts)?;
            let final_ty = if let Some(te) = ty {
                let decl_ty = checked.resolve_type(te)?;
                if !val_ty.is_assignable_to(&decl_ty) {
                    return Err(format!(
                        "Const '{}' type mismatch: expected {}, got {}",
                        name, decl_ty, val_ty
                    ));
                }
                decl_ty
            } else {
                val_ty
            };
            checked.consts.insert(name.clone(), (final_ty, val));
        }
    }
    Ok(())
}
