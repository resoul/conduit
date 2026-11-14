pub mod ast;
pub mod check;
pub(crate) mod check_call;
pub(crate) mod check_native;
pub(crate) mod check_validation;
pub mod lexer;
pub mod linker;
pub(crate) mod monomorph;
pub(crate) mod monomorph_reg;
pub(crate) mod monomorph_subst;
pub mod parser;
pub mod parser_expr;
pub mod parser_stmt;
pub mod reachability;
pub mod types;

use std::collections::HashMap;

use ast::Program;
use check::{CheckedProgram, check_program, check_program_with_roots};
use lexer::Lexer;
use linker::link_imports;
use parser::Parser;

use crate::ir::Module;
use crate::ir::lower::compile_program;

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
