mod program;
mod call;
mod expr;
mod resolve;
mod scope;
mod stmt;
mod unify;
#[cfg(test)]
mod tests;

use stmt::{block_always_exits_branch, block_always_returns};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::*;
use crate::lexer::Span;
use crate::loader::{ModuleLoader, ModuleTarget};
use crate::native::NativeRegistry;
use crate::types::{ModuleId, Type, TypeId};

#[derive(Debug, Clone, PartialEq)]
pub struct TypeError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}:{}] Type error: {}", self.span.line, self.span.col, self.message)
    }
}

impl std::error::Error for TypeError {}

#[derive(Debug, Clone)]
pub struct FuncSignature {
    pub type_params: Vec<String>,
    pub params: Vec<(String, Type)>,
    pub return_type: Type,
    pub is_exported: bool,
    pub is_validator: bool,
}

#[derive(Debug, Clone)]
pub struct StructSignature {
    pub type_params: Vec<String>,
    pub fields: HashMap<String, Type>,
    pub field_annotations: HashMap<String, Vec<crate::ast::FieldAnnotation>>,
    pub is_exported: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ModuleInfo {
    pub exports: HashSet<String>,
    pub functions: HashMap<String, FuncSignature>,
    pub structs: HashMap<String, StructSignature>,
    pub errors: HashSet<String>,
    pub imports: HashMap<String, ModuleTarget>,
}

#[derive(Debug, Clone)]
struct VarBinding {
    base_ty: Type,
    is_mutable: bool,
    span: Span,
    used: bool,
}

enum OptionalChainTarget {
    Optional(Type),
    Fallible(Vec<Type>, Vec<Type>),
    Invalid,
}

pub struct TypeChecker {
    pub native_registry: NativeRegistry,
    pub module_info: HashMap<PathBuf, ModuleInfo>,
    pub current_module_path: Option<PathBuf>,
    pub functions: HashMap<String, FuncSignature>,
    pub structs: HashMap<String, StructSignature>,
    pub errors: HashSet<String>,
    pub imports: HashMap<String, ModuleTarget>,
    scopes: Vec<HashMap<String, VarBinding>>,
    narrowed: HashMap<String, Type>,
    widened_outer_vars: Vec<(usize, HashSet<String>)>,
    closure_boundary_scope_depth: Vec<usize>,
    current_return_type: Option<Type>,
    current_type_params: Vec<String>,
    loop_depth: usize,
}

impl TypeChecker {
    pub fn new() -> Self {
        let mut checker = Self {
            native_registry: NativeRegistry::new(),
            module_info: HashMap::new(),
            current_module_path: None,
            functions: HashMap::new(),
            structs: HashMap::new(),
            errors: HashSet::new(),
            imports: HashMap::new(),
            scopes: Vec::new(),
            narrowed: HashMap::new(),
            widened_outer_vars: Vec::new(),
            closure_boundary_scope_depth: Vec::new(),
            current_return_type: None,
            current_type_params: Vec::new(),
            loop_depth: 0,
        };

        checker.functions.insert(
            "print".to_string(),
            FuncSignature {
                type_params: Vec::new(),
                params: vec![("msg".to_string(), Type::Any)],
                return_type: Type::Void,
                is_exported: false,
                is_validator: false,
            },
        );

        checker
    }

}

impl Default for TypeChecker {
    fn default() -> Self {
        Self::new()
    }
}
