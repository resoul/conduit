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

#[derive(Debug, Clone)]
pub struct CliDispatchRoute {
    pub field_slot_path: Vec<u16>,
    pub handler_name: String,
    pub takes_parent: bool,
    pub ret_type: Type,
}

#[derive(Debug, Clone)]
pub enum HttpRouterDeclKind {
    Before,
    After,
    GroupBefore(String),
    GroupAfter(String),
    Static(String, String),
    Cors(String),
    OnStart,
    OnStop,
    Route(String, String),
}

#[derive(Debug, Clone)]
pub struct CronScheduleEntry {
    pub name: String,
    pub cron_expr: Option<String>,
    pub interval_ms: Option<u64>,
    pub handler_name: String,
}

#[derive(Debug, Clone)]
pub struct HttpRouterDeclEntry {
    pub kind: HttpRouterDeclKind,
    pub handler_name: String,
    pub query_struct: Option<String>,
    pub body_struct: Option<String>,
    pub guard_handler: Option<String>,
    pub auto_response: bool,
}

#[derive(Debug, Clone)]
pub struct DataEnumVariant {
    pub name: String,
    pub struct_name: String,
    pub schema_id: u32,
}

#[derive(Debug)]
pub struct CheckedProgram {
    pub structs: Vec<ResolvedStruct>,
    pub struct_by_name: HashMap<String, usize>,
    pub funcs: Vec<FuncSig>,
    pub func_by_name: HashMap<String, u32>,
    pub struct_schemas: Vec<StructSchema>,
    pub native_aliases: HashMap<String, &'static str>,
    pub native_direct_fns: HashMap<String, (&'static str, String)>,
    pub native_direct_types: HashMap<String, (&'static str, String)>,
    pub env_plans: Vec<crate::native::env::EnvLoadPlan>,
    pub cli_plans: Vec<(String, crate::native::cli::CliPlan)>,
    pub cli_dispatches: Vec<(String, Vec<CliDispatchRoute>)>,
    pub http_router_plans: Vec<(String, Vec<HttpRouterDeclEntry>)>,
    pub db_model_plans: Vec<crate::ir::DbModelMeta>,
    pub cron_schedule_plans: Vec<(String, Vec<CronScheduleEntry>)>,
    pub struct_abouts: HashMap<String, String>,
    pub validator_funcs: HashSet<String>,
    pub error_structs: HashSet<String>,
    pub http_query_structs: HashSet<String>,
    pub http_status_by_struct: HashMap<String, u16>,
    pub validation_plans: Vec<ValidationStructPlan>,
    pub generic_structs: HashMap<String, GenericStructTemplate>,
    pub generic_funcs: HashMap<String, GenericFuncTemplate>,
    pub mono_struct_args: HashMap<String, (String, Vec<Type>)>,
    pub pending_mono_items: Vec<Item>,
    pub consts: HashMap<String, (Type, ConstVal)>,
    pub enums: HashMap<String, Vec<String>>,
    pub data_enums: HashMap<String, Vec<DataEnumVariant>>,
}

impl CheckedProgram {
    pub fn resolve_type(&self, te: &TypeExpr) -> Result<Type, String> {
        resolve_type_expr(te, self)
    }

    pub(crate) fn resolve_type_mut(&mut self, te: &TypeExpr) -> Result<Type, String> {
        let empty = HashSet::new();
        resolve_type_expr_with_scope(te, self, &empty)
    }

    pub fn is_native_alias(&self, alias: &str, module: &str) -> bool {
        self.native_aliases.get(alias).copied() == Some(module)
    }

    pub fn native_direct_fn(&self, name: &str, module: &str) -> Option<&str> {
        self.native_direct_fns
            .get(name)
            .filter(|(m, _)| *m == module)
            .map(|(_, sym)| sym.as_str())
    }
}

fn with_span(span: crate::frontend::lexer::Span, err: String) -> String {
    if err.starts_with('[') || span.line == 0 {
        err
    } else {
        format!("[{}:{}] {}", span.line, span.col, err)
    }
}

pub(crate) fn format_check_err(
    span: crate::frontend::lexer::Span,
    source_file: Option<&str>,
    err: String,
) -> String {
    let msg = with_span(span, err);
    if let Some(f) = source_file {
        crate::diagnostics::tag_file_error(std::path::Path::new(f), msg)
    } else {
        msg
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
        native_aliases: HashMap::from([("json".to_string(), "cdt.json")]),
        native_direct_fns: HashMap::new(),
        native_direct_types: HashMap::new(),
        env_plans: Vec::new(),
        cli_plans: Vec::new(),
        cli_dispatches: Vec::new(),
        http_router_plans: Vec::new(),
        db_model_plans: Vec::new(),
        cron_schedule_plans: Vec::new(),
        struct_abouts: HashMap::new(),
        validator_funcs: HashSet::new(),
        error_structs: HashSet::new(),
        http_query_structs: HashSet::new(),
        http_status_by_struct: HashMap::new(),
        validation_plans: Vec::new(),
        generic_structs: HashMap::new(),
        generic_funcs: HashMap::new(),
        mono_struct_args: HashMap::new(),
        pending_mono_items: Vec::new(),
        consts: HashMap::new(),
        enums: HashMap::new(),
        data_enums: HashMap::new(),
    };

    register_native_imports(program, &mut checked)?;
    register_program_types(program, &mut checked)?;
    register_program_consts(program, &mut checked)?;
    check_validator_contracts(program, &mut checked)?;
    let mut all_errors: Vec<String> = Vec::new();
    check_default_exprs(program, &mut checked, &mut all_errors);
    crate::frontend::check_query::check_and_desugar_queries(program, &mut checked, &mut all_errors);
    check_and_desugar_jobs(program, &mut checked, &mut all_errors);

    let mut idx = 0usize;
    while idx < program.items.len() {
        let is_concrete_fn = matches!(
            &program.items[idx].kind,
            ItemKind::Func { type_params, .. } if type_params.is_empty()
        );
        if is_concrete_fn {
            let source_file = program.items[idx].source_file.clone();
            let item_span = program.items[idx].span;
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
            if crate::ir::is_lifecycle_hook_name(name)
                && !crate::frontend::check_sys::is_cron_handler_return(&sig.return_type, &checked)
            {
                all_errors.push(format_check_err(
                    item_span,
                    source_file.as_deref(),
                    format!(
                        "on start / on stop hook must return Void or Void | Error, got {}",
                        sig.return_type
                    ),
                ));
            }
            let mut tc = Checker {
                checked: &mut checked,
                scopes: vec![HashMap::new()],
                current_return: sig.return_type.clone(),
                loop_depth: 0,
                in_defer: false,
                errors: Vec::new(),
                source_file,
            };
            for (param, ty) in params.iter().zip(sig.params.iter()) {
                tc.bind(param.name.clone(), ty.clone());
            }
            let _ = tc.check_block(body);
            all_errors.extend(tc.errors);
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

    let mut seen_mig_versions = HashSet::new();
    let db_conn_ty = Type::Custom(TypeId::native("cdt.db", "Connection"));
    let db_err_ty = Type::Custom(TypeId::native("cdt.db", "DbError"));
    let mig_ret_ty = Type::union(vec![Type::Void, db_err_ty]);
    for item in &mut program.items {
        let source_file = item.source_file.clone();
        let item_span = item.span;
        if let ItemKind::Migration { version, up, down, .. } = &mut item.kind {
            if !seen_mig_versions.insert(*version) {
                all_errors.push(format_check_err(
                    item_span,
                    source_file.as_deref(),
                    format!("Duplicate migration version {}", version),
                ));
            }
            for block in [up, down] {
                let mut tc = Checker {
                    checked: &mut checked,
                    scopes: vec![HashMap::new()],
                    current_return: mig_ret_ty.clone(),
                    loop_depth: 0,
                    in_defer: false,
                    errors: Vec::new(),
                    source_file: source_file.clone(),
                };
                tc.bind("db".to_string(), db_conn_ty.clone());
                let _ = tc.check_block(block);
                all_errors.extend(tc.errors);
            }
        }
    }
    let mut seen_test_names = HashSet::new();
    for item in &mut program.items {
        let source_file = item.source_file.clone();
        let item_span = item.span;
        if let ItemKind::Test { name, body } = &mut item.kind {
            if !seen_test_names.insert(name.clone()) {
                all_errors.push(format_check_err(
                    item_span,
                    source_file.as_deref(),
                    format!("Duplicate test name '{}'", name),
                ));
            }
            let mut tc = Checker {
                checked: &mut checked,
                scopes: vec![HashMap::new()],
                current_return: Type::Void,
                loop_depth: 0,
                in_defer: false,
                errors: Vec::new(),
                source_file,
            };
            let _ = tc.check_block(body);
            all_errors.extend(tc.errors);
        }
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

    if let Err(e) = build_validation_plans(program, &mut checked) {
        all_errors.push(e);
    }

    let mut struct_locs: HashMap<String, (crate::frontend::lexer::Span, Option<String>)> =
        HashMap::new();
    for item in &program.items {
        if let ItemKind::Struct { name, .. } = &item.kind {
            struct_locs.insert(name.clone(), (item.span, item.source_file.clone()));
        }
    }

    let router_names: Vec<String> = checked
        .structs
        .iter()
        .filter(|s| {
            s.fields.iter().any(|f| {
                f.router_ann
                    .as_ref()
                    .is_some_and(|a| a.target.starts_with("router."))
            })
        })
        .map(|s| s.name.clone())
        .collect();
    let req_ty = Type::Custom(TypeId::native("cdt.http", "Request"));
    let resp_ty = Type::Custom(TypeId::native("cdt.http", "Response"));
    for rname in router_names {
        if let Err(e) =
            crate::frontend::check_sys::ensure_http_router_plan(&mut checked, &rname, &req_ty, &resp_ty)
        {
            let (sp, sf) = struct_locs
                .get(&rname)
                .cloned()
                .unwrap_or((crate::frontend::lexer::Span::new(0, 0), None));
            all_errors.push(format_check_err(sp, sf.as_deref(), e));
        }
    }

    let schedule_names: Vec<String> = checked
        .structs
        .iter()
        .filter(|s| {
            s.fields.iter().any(|f| {
                f.router_ann
                    .as_ref()
                    .is_some_and(|a| a.target.starts_with("schedule."))
            })
        })
        .map(|s| s.name.clone())
        .collect();
    for sname in schedule_names {
        if let Err(e) = crate::frontend::check_sys::ensure_cron_schedule_plan(&mut checked, &sname) {
            let (sp, sf) = struct_locs
                .get(&sname)
                .cloned()
                .unwrap_or((crate::frontend::lexer::Span::new(0, 0), None));
            all_errors.push(format_check_err(sp, sf.as_deref(), e));
        }
    }
    let db_model_names: Vec<String> = checked
        .structs
        .iter()
        .filter(|s| s.db_table.is_some() || s.fields.iter().any(|f| !f.db_anns.is_empty()))
        .map(|s| s.name.clone())
        .collect();
    for mname in db_model_names {
        if let Err(e) = crate::frontend::check_sys::ensure_db_model_plan(&mut checked, &mname) {
            let (sp, sf) = struct_locs
                .get(&mname)
                .cloned()
                .unwrap_or((crate::frontend::lexer::Span::new(0, 0), None));
            all_errors.push(format_check_err(sp, sf.as_deref(), e));
        }
    }

    if !all_errors.is_empty() {
        let mut seen_errs = HashSet::new();
        all_errors.retain(|e| seen_errs.insert(e.clone()));
        return Err(all_errors.join("\n"));
    }

    shake_unreachable_funcs(program, &mut checked, explicit_roots)?;
    finalize_validation_plan_func_ids(&mut checked);
    Ok(checked)
}

fn check_param_defaults(
    name: &str,
    params: &mut [Param],
    checked: &mut CheckedProgram,
    source_file: &Option<String>,
    all_errors: &mut Vec<String>,
) {
    let fid = checked.func_by_name[name] as usize;
    for (i, p) in params.iter_mut().enumerate() {
        if let Some(def_expr) = &mut p.default {
            let expected = checked.funcs[fid].params[i].clone();
            let mut tc = Checker {
                checked,
                scopes: vec![HashMap::new()],
                current_return: Type::Void,
                loop_depth: 0,
                in_defer: false,
                errors: Vec::new(),
                source_file: source_file.clone(),
            };
            let res = tc.check_expr(def_expr, Some(&expected));
            all_errors.extend(tc.errors);
            if let Err(e) = res {
                all_errors.push(format_check_err(p.span, source_file.as_deref(), e));
            } else if !def_expr.ty.is_assignable_to(&expected) {
                all_errors.push(format_check_err(
                    def_expr.span,
                    source_file.as_deref(),
                    format!(
                        "Default value for parameter '{}' of '{}' expected {}, got {}",
                        p.name, name, expected, def_expr.ty
                    ),
                ));
            } else {
                checked.funcs[fid].defaults[i] = Some(def_expr.clone());
            }
        }
    }
}

fn check_default_exprs(
    program: &mut Program,
    checked: &mut CheckedProgram,
    all_errors: &mut Vec<String>,
) {
    for item in &mut program.items {
        let source_file = item.source_file.clone();
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
                            in_defer: false,
                            errors: Vec::new(),
                            source_file: source_file.clone(),
                        };
                        let res = tc.check_expr(def_expr, Some(&expected));
                        all_errors.extend(tc.errors);
                        if let Err(e) = res {
                            all_errors.push(format_check_err(
                                f.span,
                                source_file.as_deref(),
                                e,
                            ));
                        } else if !def_expr.ty.is_assignable_to(&expected) {
                            all_errors.push(format_check_err(
                                def_expr.span,
                                source_file.as_deref(),
                                format!(
                                    "Default value for field '{}.{}' expected {}, got {}",
                                    name, f.name, expected, def_expr.ty
                                ),
                            ));
                        } else {
                            checked.structs[sid].fields[i].default = Some(def_expr.clone());
                        }
                    }
                }
            }
            ItemKind::Func { name, type_params, params, .. } if type_params.is_empty() => {
                check_param_defaults(name, params, checked, &source_file, all_errors);
            }
            ItemKind::Query(q) => {
                check_param_defaults(&q.name, &mut q.params, checked, &source_file, all_errors);
            }
            ItemKind::Job(_) => {}
            ItemKind::Enum { name, variants } => {
                if let Some(dvars) = checked.data_enums.get(name).cloned() {
                    for (vast, dv) in variants.iter_mut().zip(dvars.iter()) {
                        let sid = checked.struct_by_name[&dv.struct_name];
                        for (i, f) in vast.fields.iter_mut().enumerate() {
                            if let Some(def_expr) = &mut f.default {
                                let expected = checked.structs[sid].fields[i].ty.clone();
                                let mut tc = Checker {
                                    checked,
                                    scopes: vec![HashMap::new()],
                                    current_return: Type::Void,
                                    loop_depth: 0,
                                    in_defer: false,
                                    errors: Vec::new(),
                                    source_file: source_file.clone(),
                                };
                                let res = tc.check_expr(def_expr, Some(&expected));
                                all_errors.extend(tc.errors);
                                if let Err(e) = res {
                                    all_errors.push(format_check_err(
                                        f.span,
                                        source_file.as_deref(),
                                        e,
                                    ));
                                } else if !def_expr.ty.is_assignable_to(&expected) {
                                    all_errors.push(format_check_err(
                                        def_expr.span,
                                        source_file.as_deref(),
                                        format!(
                                            "Default value for field '{}.{}.{}' expected {}, got {}",
                                            name, vast.name, f.name, expected, def_expr.ty
                                        ),
                                    ));
                                } else {
                                    checked.structs[sid].fields[i].default = Some(def_expr.clone());
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn check_and_desugar_jobs(
    program: &mut Program,
    checked: &mut CheckedProgram,
    all_errors: &mut Vec<String>,
) {
    let mut job_entries = Vec::new();
    for item in &mut program.items {
        let source_file = item.source_file.clone();
        let ItemKind::Job(_) = &item.kind else {
            continue;
        };
        let placeholder = ItemKind::ExportList { names: Vec::new() };
        let ItemKind::Job(job) = std::mem::replace(&mut item.kind, placeholder) else {
            continue;
        };

        if let Some(expr) = &job.cron_expr
            && let Err(e) = crate::native::cron::parse_cron_expr(expr)
        {
            all_errors.push(format_check_err(
                job.expr_span,
                source_file.as_deref(),
                format!("Invalid cron expression '{}' in job '{}': {}", expr, job.name, e),
            ));
        }

        if let Some(&fid) = checked.func_by_name.get(&job.name) {
            let ret_ty = &checked.funcs[fid as usize].return_type;
            if !crate::frontend::check_sys::is_cron_handler_return(ret_ty, checked) {
                all_errors.push(format_check_err(
                    item.span,
                    source_file.as_deref(),
                    format!(
                        "job '{}' must have return type Void or Void | Error, got {}",
                        job.name, ret_ty
                    ),
                ));
            }
        }

        job_entries.push(CronScheduleEntry {
            name: job.name.clone(),
            cron_expr: job.cron_expr,
            interval_ms: job.interval_ms,
            handler_name: job.name.clone(),
        });

        item.kind = ItemKind::Func {
            name: job.name,
            type_params: Vec::new(),
            params: Vec::new(),
            return_type: job.return_type,
            body: job.body,
            is_validator: false,
        };
    }

    if !job_entries.is_empty() {
        if let Some((_, existing)) = checked
            .cron_schedule_plans
            .iter_mut()
            .find(|(n, _)| n == "Jobs")
        {
            existing.extend(job_entries);
        } else {
            checked.cron_schedule_plans.push(("Jobs".to_string(), job_entries));
        }
    }
}

pub(crate) type ResolvedDataEnumBindings<'a> = (
    &'a DataEnumVariant,
    Vec<(String, &'a crate::frontend::types::ResolvedField)>,
);

pub(crate) fn resolve_data_enum_bindings<'a>(
    checked: &'a CheckedProgram,
    ename: &str,
    vname: &str,
    bindings: &[(Option<String>, String)],
) -> Result<ResolvedDataEnumBindings<'a>, String> {
    let vars = checked
        .data_enums
        .get(ename)
        .ok_or_else(|| format!("Unknown data enum '{}'", ename))?;
    let var = vars
        .iter()
        .find(|v| v.name == vname)
        .ok_or_else(|| format!("Enum '{}' has no variant '{}'", ename, vname))?;
    let sid = checked.struct_by_name[&var.struct_name];
    let fields = &checked.structs[sid].fields;

    if fields.is_empty() && !bindings.is_empty() {
        return Err(format!(
            "Unit enum variant '{}.{}' does not have payload fields",
            ename, vname
        ));
    }

    let mut out = Vec::with_capacity(bindings.len());
    let mut seen_locals = HashSet::new();
    let all_positional = bindings.iter().all(|(opt, _)| opt.is_none());

    for (pos, (opt_fname, local)) in bindings.iter().enumerate() {
        let target_field = if let Some(fname) = opt_fname {
            fields.iter().find(|f| &f.name == fname).ok_or_else(|| {
                format!("Enum variant '{}.{}' has no field '{}'", ename, vname, fname)
            })?
        } else if local == "_" {
            if all_positional && pos < fields.len() {
                &fields[pos]
            } else {
                continue;
            }
        } else if let Some(by_name) = fields.iter().find(|f| &f.name == local) {
            by_name
        } else if all_positional && bindings.len() == fields.len() {
            &fields[pos]
        } else {
            return Err(format!(
                "Enum variant '{}.{}' has no field '{}'",
                ename, vname, local
            ));
        };

        if local != "_" {
            if !seen_locals.insert(local.clone()) {
                return Err(format!(
                    "Duplicate binding variable '{}' in pattern '{}.{}'",
                    local, ename, vname
                ));
            }
            out.push((local.clone(), target_field));
        }
    }

    Ok((var, out))
}

pub(crate) struct Checker<'a> {
    pub(crate) checked: &'a mut CheckedProgram,
    pub(crate) scopes: Vec<HashMap<String, (Type, bool)>>,
    pub(crate) current_return: Type,
    pub(crate) loop_depth: usize,
    pub(crate) in_defer: bool,
    pub(crate) errors: Vec<String>,
    pub(crate) source_file: Option<String>,
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
            top.insert(name, (ty, false));
        }
    }

    pub(crate) fn bind_mut(&mut self, name: String, ty: Type) {
        if let Some(top) = self.scopes.last_mut() {
            top.insert(name, (ty, true));
        }
    }

    pub(crate) fn is_local(&self, name: &str) -> bool {
        self.scopes.iter().any(|scope| scope.contains_key(name))
    }

    pub(crate) fn lookup_mutability(&self, name: &str) -> Option<bool> {
        for scope in self.scopes.iter().rev() {
            if let Some((_, is_mut)) = scope.get(name) {
                return Some(*is_mut);
            }
        }
        None
    }

    pub(crate) fn lookup(&self, name: &str) -> Option<Type> {
        for scope in self.scopes.iter().rev() {
            if let Some((ty, _)) = scope.get(name) {
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

    fn bind_pattern_any(&mut self, pat: &Pattern) {
        match &pat.kind {
            PatternKind::Ident(name) => {
                if self.lookup(name).is_none() {
                    self.bind(name.clone(), Type::Any);
                }
            }
            PatternKind::Tuple(pats) => {
                for p in pats {
                    self.bind_pattern_any(p);
                }
            }
            PatternKind::EnumVariant { bindings, .. } => {
                for (_, var) in bindings {
                    if var != "_" && self.lookup(var).is_none() {
                        self.bind(var.clone(), Type::Any);
                    }
                }
            }
            PatternKind::Wildcard | PatternKind::Nil | PatternKind::Type { .. } => {}
        }
    }

    pub(crate) fn record_stmt_error(&mut self, stmt: &Stmt, err: String) {
        self.errors.push(format_check_err(
            stmt.span,
            self.source_file.as_deref(),
            err,
        ));
        match &stmt.kind {
            StmtKind::Define { name, .. } if self.lookup(name).is_none() => {
                self.bind(name.clone(), Type::Any);
            }
            StmtKind::DestructureDefine { pattern, .. } => {
                self.bind_pattern_any(pattern);
            }
            StmtKind::VarDecl { name, ty, .. } if self.lookup(name).is_none() => {
                let fallback = ty
                    .as_ref()
                    .and_then(|te| self.checked.resolve_type(te).ok())
                    .unwrap_or(Type::Any);
                self.bind_mut(name.clone(), fallback);
            }
            _ => {}
        }
    }

    fn check_block(&mut self, block: &mut Block) -> Result<(), String> {
        self.push_scope();
        for stmt in &mut block.stmts {
            if let Err(e) = self.check_stmt(stmt) {
                self.record_stmt_error(stmt, e);
            }
        }
        self.pop_scope();
        Ok(())
    }

    pub(crate) fn check_stmt(&mut self, stmt: &mut Stmt) -> Result<(), String> {
        let span = stmt.span;
        self.check_stmt_inner(stmt).map_err(|e| with_span(span, e))
    }

    fn check_stmt_inner(&mut self, stmt: &mut Stmt) -> Result<(), String> {
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
                        self.bind_mut(name.clone(), expected.clone());
                    } else {
                        self.bind_mut(name.clone(), init_expr.ty.clone());
                    }
                } else if let Some(expected) = decl_ty {
                    self.bind_mut(name.clone(), expected);
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
                if let ExprKind::Ident(name) = &target.kind {
                    if self.lookup(name).is_none()
                        && self.checked.consts.contains_key(name)
                    {
                        return Err(format!("Cannot assign to constant '{}'", name));
                    }
                    if self.lookup_mutability(name) == Some(false) {
                        return Err(format!(
                            "Cannot assign to immutable variable '{}' (declare with 'var' to allow reassignment)",
                            name
                        ));
                    }
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
                if self.in_defer {
                    return Err("Cannot use 'return' inside 'defer'".to_string());
                }
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
                    Type::Int => Type::Int,
                    Type::Generic { type_id, type_args }
                        if type_id.module == ModuleId::Native("cdt.chan".to_string())
                            && type_id.name == "Chan" =>
                    {
                        type_args.first().cloned().unwrap_or(Type::Any)
                    }
                    other => return Err(format!("Cannot iterate over {}", other)),
                };
                self.loop_depth += 1;
                self.push_scope();
                self.bind_pattern(pattern, &elem_ty)?;
                for s in &mut body.stmts {
                    if let Err(e) = self.check_stmt(s) {
                        self.record_stmt_error(s, e);
                    }
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
                let mut has_catch_all = false;
                let mut covered_variants = HashSet::new();
                for case in cases {
                    match &case.pattern.kind {
                        PatternKind::Wildcard => has_catch_all = true,
                        PatternKind::Ident(n) if !n.contains('.') => has_catch_all = true,
                        PatternKind::Ident(n) => {
                            if let Some((_, rhs)) = n.split_once('.') {
                                covered_variants.insert(rhs.to_string());
                            }
                        }
                        PatternKind::Type { name, .. } => {
                            if let Some((_, rhs)) = name.rsplit_once('.') {
                                covered_variants.insert(rhs.to_string());
                            } else if let Type::DataEnum(ename) = &expr.ty
                                && name == ename
                            {
                                has_catch_all = true;
                            }
                        }
                        PatternKind::EnumVariant { variant_name, .. } => {
                            covered_variants.insert(variant_name.clone());
                        }
                        _ => {}
                    }
                    self.push_scope();
                    self.bind_switch_pattern(&case.pattern, &expr.ty)?;
                    for s in &mut case.body.stmts {
                        if let Err(e) = self.check_stmt(s) {
                            self.record_stmt_error(s, e);
                        }
                    }
                    self.pop_scope();
                }
                if let Type::DataEnum(ename) = &expr.ty
                    && !has_catch_all
                    && let Some(dvars) = self.checked.data_enums.get(ename)
                {
                    let missing: Vec<&str> = dvars
                        .iter()
                        .filter(|v| !covered_variants.contains(&v.name))
                        .map(|v| v.name.as_str())
                        .collect();
                    if !missing.is_empty() {
                        return Err(format!(
                            "Non-exhaustive switch on enum '{}': missing variant(s) {}",
                            ename,
                            missing.join(", ")
                        ));
                    }
                }
            }
            StmtKind::Defer(block) => {
                if self.in_defer {
                    return Err("Cannot nest 'defer' inside 'defer'".to_string());
                }
                let prev_loop = self.loop_depth;
                self.loop_depth = 0;
                self.in_defer = true;
                let _ = self.check_block(block);
                self.in_defer = false;
                self.loop_depth = prev_loop;
            }
            StmtKind::Spawn(block) => {
                let prev_loop = self.loop_depth;
                let prev_defer = self.in_defer;
                let prev_ret = std::mem::replace(&mut self.current_return, Type::Void);
                self.loop_depth = 0;
                self.in_defer = false;
                let _ = self.check_block(block);
                self.current_return = prev_ret;
                self.in_defer = prev_defer;
                self.loop_depth = prev_loop;
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
        if let PatternKind::Ident(pat_name) = &pat.kind
            && let Some((lhs, rhs)) = pat_name.split_once('.')
        {
            if let Type::Enum(ename) = scrutinee_ty {
                if lhs != ename {
                    return Err(format!(
                        "Enum pattern '{}' does not match enum type '{}'",
                        pat_name, ename
                    ));
                }
                let variants = self.checked.enums.get(ename).ok_or_else(|| {
                    format!("Unknown enum '{}'", ename)
                })?;
                if !variants.iter().any(|v| v == rhs) {
                    return Err(format!("Enum '{}' has no variant '{}'", ename, rhs));
                }
                return Ok(());
            }
            if let Type::DataEnum(ename) = scrutinee_ty {
                if lhs != ename {
                    return Err(format!(
                        "Enum pattern '{}' does not match enum type '{}'",
                        pat_name, ename
                    ));
                }
                let (var, _) = resolve_data_enum_bindings(self.checked, ename, rhs, &[])?;
                let sid = self.checked.struct_by_name[&var.struct_name];
                if !self.checked.structs[sid].fields.is_empty() {
                    return Err(format!(
                        "Variant '{}.{}' has payload fields; match with 'case {}.{}({{...}}):' or 'case {}.{}(_):'",
                        ename, rhs, ename, rhs, ename, rhs
                    ));
                }
                return Ok(());
            }
        }
        if let PatternKind::Type { name, binding } = &pat.kind {
            if let Type::DataEnum(ename) = scrutinee_ty
                && let Some((lhs, rhs)) = name.rsplit_once('.')
            {
                if lhs != ename {
                    return Err(format!(
                        "Enum pattern '{}' does not match enum type '{}'",
                        name, ename
                    ));
                }
                let (_, resolved_fields) = resolve_data_enum_bindings(
                    self.checked,
                    ename,
                    rhs,
                    &[(None, binding.clone())],
                )?;
                let to_bind: Vec<(String, Type)> = resolved_fields
                    .into_iter()
                    .map(|(local, f)| (local, f.ty.clone()))
                    .collect();
                for (local, ty) in to_bind {
                    self.bind(local, ty);
                }
                return Ok(());
            }
            let resolved = resolve_switch_pattern_type(name, scrutinee_ty, self.checked)?;
            if binding != "_" {
                self.bind(binding.clone(), resolved);
            }
            Ok(())
        } else if let PatternKind::EnumVariant {
            enum_name,
            variant_name,
            bindings,
        } = &pat.kind
        {
            let Type::DataEnum(ename) = scrutinee_ty else {
                return Err(format!(
                    "Variant pattern '{}.{}' requires enum type, got {}",
                    enum_name, variant_name, scrutinee_ty
                ));
            };
            if enum_name != ename {
                return Err(format!(
                    "Enum pattern '{}.{}' does not match enum type '{}'",
                    enum_name, variant_name, ename
                ));
            }
            let (_, resolved_fields) =
                resolve_data_enum_bindings(self.checked, ename, variant_name, bindings)?;
            let to_bind: Vec<(String, Type)> = resolved_fields
                .into_iter()
                .map(|(local, f)| (local, f.ty.clone()))
                .collect();
            for (local, ty) in to_bind {
                self.bind(local, ty);
            }
            Ok(())
        } else {
            self.bind_pattern(pat, scrutinee_ty)
        }
    }

    pub(crate) fn check_expr(&mut self, expr: &mut Expr, expected: Option<&Type>) -> Result<(), String> {
        let span = expr.span;
        self.check_expr_inner(expr, expected).map_err(|e| with_span(span, e))
    }

    fn check_expr_inner(&mut self, expr: &mut Expr, expected: Option<&Type>) -> Result<(), String> {
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
                } else if self.checked.native_direct_fn(name, "cdt.validation") == Some("email") {
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
                if self.in_defer {
                    return Err("Cannot use '?' inside 'defer'; handle errors explicitly with 'switch'".to_string());
                }
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
                if let ExprKind::Ident(ename) = &target.kind
                    && self.lookup(ename).is_none()
                    && let Some(variants) = self.checked.enums.get(ename)
                {
                    let idx = variants
                        .iter()
                        .position(|v| v == field)
                        .ok_or_else(|| format!("Enum '{}' has no variant '{}'", ename, field))?;
                    const_inline = Some(ExprKind::Int(idx as i64));
                    Type::Enum(ename.clone())
                } else if let ExprKind::Ident(ename) = &target.kind
                    && self.lookup(ename).is_none()
                    && let Some(dvars) = self.checked.data_enums.get(ename)
                {
                    let var = dvars
                        .iter()
                        .find(|v| &v.name == field)
                        .ok_or_else(|| format!("Enum '{}' has no variant '{}'", ename, field))?;
                    let sid = self.checked.struct_by_name[&var.struct_name];
                    if !self.checked.structs[sid].fields.is_empty() {
                        return Err(format!(
                            "Enum variant '{}.{}' requires arguments",
                            ename, field
                        ));
                    }
                    Type::DataEnum(ename.clone())
                } else if let ExprKind::Ident(alias) = &target.kind
                    && self.lookup(alias).is_none()
                    && self.checked.is_native_alias(alias, "cdt.validation")
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
                    ty @ (Type::List(_) | Type::Bytes) => {
                        let elem = match ty {
                            Type::List(inner) => (**inner).clone(),
                            _ => Type::Int,
                        };
                        self.check_expr(index, Some(&Type::Int))?;
                        if index.ty != Type::Int {
                            return Err(format!("List index must be Int, got {}", index.ty));
                        }
                        Type::union(vec![
                            elem,
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
                (BinaryOp::BitAnd, ConstVal::Int(a), ConstVal::Int(b)) => {
                    Ok((Type::Int, ConstVal::Int(a & b)))
                }
                (BinaryOp::BitOr, ConstVal::Int(a), ConstVal::Int(b)) => {
                    Ok((Type::Int, ConstVal::Int(a | b)))
                }
                (BinaryOp::BitXor, ConstVal::Int(a), ConstVal::Int(b)) => {
                    Ok((Type::Int, ConstVal::Int(a ^ b)))
                }
                (BinaryOp::ShiftLeft | BinaryOp::ShiftRight, ConstVal::Int(a), ConstVal::Int(b)) => {
                    let shift = u32::try_from(b).ok().filter(|&b| b < i64::BITS)
                        .ok_or_else(|| "Shift count must be in 0..64".to_owned())?;
                    let value = if *op == BinaryOp::ShiftLeft { a << shift } else { a >> shift };
                    Ok((Type::Int, ConstVal::Int(value)))
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
