pub mod ast;
pub mod check;
pub(crate) mod check_call;
pub(crate) mod check_cli;
pub(crate) mod check_fs;
pub(crate) mod check_native;
pub(crate) mod check_sys;
pub(crate) mod check_validation;
pub mod lexer;
pub mod linker;
pub(crate) mod monomorph;
pub(crate) mod monomorph_reg;
pub(crate) mod monomorph_subst;
pub mod parser;
pub(crate) mod parser_cli;
pub mod parser_expr;
pub mod parser_stmt;
pub mod reachability;
pub mod types;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use ast::{ItemKind, Program};
use check::{CheckedProgram, check_program, check_program_with_roots};
use lexer::Lexer;
use linker::link_imports;
use parser::Parser;

use crate::ir::Module;
use crate::ir::lower::compile_program;

fn is_native_mod(path: &str) -> bool {
    matches!(
        path,
        "cdt.json"
            | "cdt.time"
            | "cdt.validation"
            | "cdt.env"
            | "cdt.cli"
            | "cdt.fs"
            | "cdt.path"
            | "cdt.process"
            | "cdt.crypto"
            | "cdt.url"
            | "cdt.csv"
            | "cdt.math"
            | "cdt.log"
    )
}

fn resolve_import_file(base_dir: &Path, root_dir: &Path, imp: &str) -> Result<PathBuf, String> {
    let rel = imp.strip_prefix("./").unwrap_or(imp);
    let mut candidates = vec![base_dir.join(rel), root_dir.join(rel)];
    if !rel.ends_with(".cdt") {
        let with_ext = format!("{}.cdt", rel);
        candidates.insert(1, base_dir.join(&with_ext));
        candidates.push(root_dir.join(&with_ext));
    }
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| format!("Cannot find imported module '{}'", imp))
}

fn read_with_overrides(
    path: &Path,
    overrides: &HashMap<std::path::PathBuf, String>,
) -> Result<String, String> {
    if let Some(s) = overrides.get(path) {
        return Ok(s.clone());
    }
    if let Ok(canon) = path.canonicalize()
        && let Some(s) = overrides.get(&canon)
    {
        return Ok(s.clone());
    }
    fs::read_to_string(path).map_err(|e| format!("Failed to read '{}': {}", path.display(), e))
}

fn collect_file_imports(
    prog: &Program,
    cur_dir: &Path,
    root_dir: &Path,
    overrides: &HashMap<std::path::PathBuf, String>,
    seen: &mut HashSet<String>,
    out: &mut HashMap<String, String>,
) -> Result<(), String> {
    for item in &prog.items {
        let ItemKind::Import { path: imp, .. } = &item.kind else {
            continue;
        };
        if is_native_mod(imp) || !seen.insert(imp.clone()) {
            continue;
        }
        let target = resolve_import_file(cur_dir, root_dir, imp)?;
        let src = read_with_overrides(&target, overrides)?;
        let sub_prog = parse_source(&src)?;
        let sub_dir = target.parent().unwrap_or(cur_dir);
        collect_file_imports(&sub_prog, sub_dir, root_dir, overrides, seen, out)?;
        out.insert(imp.clone(), src);
    }
    Ok(())
}

pub fn check_file(path: &Path) -> Result<(Program, CheckedProgram), String> {
    check_file_with_overrides(path, &HashMap::new())
}

pub fn check_file_with_overrides(
    path: &Path,
    overrides: &HashMap<std::path::PathBuf, String>,
) -> Result<(Program, CheckedProgram), String> {
    let src = read_with_overrides(path, overrides)?;
    let mut program = parse_source(&src)?;
    let root_dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut owned_mods = HashMap::new();
    let mut seen = HashSet::new();
    collect_file_imports(&program, root_dir, root_dir, overrides, &mut seen, &mut owned_mods)?;
    if !owned_mods.is_empty() {
        let mod_refs: HashMap<&str, &str> =
            owned_mods.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        link_imports(&mut program, &mod_refs)?;
    }
    let checked = check_program(&mut program)?;
    Ok((program, checked))
}

pub fn compile_file(path: &Path) -> Result<Module, String> {
    let (program, checked) = check_file(path)?;
    let module = compile_program(&program, &checked)?;
    module.validate()?;
    Ok(module)
}

pub fn parse_source(src: &str) -> Result<Program, String> {
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize_all().map_err(|e| e.to_string())?;
    let mut parser = Parser::new(tokens);
    parser.parse_program().map_err(|e| e.to_string())
}

pub fn check_source(src: &str) -> Result<(Program, CheckedProgram), String> {
    let mut program = parse_source(src)?;
    let checked = check_program(&mut program)?;
    Ok((program, checked))
}

pub fn compile_source(src: &str) -> Result<Module, String> {
    let (program, checked) = check_source(src)?;
    let module = compile_program(&program, &checked)?;
    module.validate()?;
    Ok(module)
}

pub fn compile_source_roots(src: &str, roots: &[&str]) -> Result<Module, String> {
    let mut program = parse_source(src)?;
    let checked = check_program_with_roots(&mut program, Some(roots))?;
    let module = compile_program(&program, &checked)?;
    module.validate()?;
    Ok(module)
}

pub fn compile_with_modules(
    src: &str,
    modules: &HashMap<&str, &str>,
) -> Result<Module, String> {
    let mut program = parse_source(src)?;
    link_imports(&mut program, modules)?;
    let checked = check_program(&mut program)?;
    let module = compile_program(&program, &checked)?;
    module.validate()?;
    Ok(module)
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_native;
#[cfg(test)]
mod tests_generics;
