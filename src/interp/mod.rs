mod call;
mod env;
mod expr;
mod json;
mod methods;
mod pattern;
mod stmt;
mod types;
mod validation;
#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::ast::*;
use crate::lexer::{ESCAPED_CLOSE_BRACE, ESCAPED_OPEN_BRACE, Span};
use crate::loader::{ModuleLoader, ModuleTarget};
use crate::native::json::{
    decode_json_with_plan, encode_with_struct_schemas, make_decode_error, make_encode_error,
    JsonDecodePlan,
};
use crate::native::NativeRegistry;
use crate::types::{ModuleId, Type, TypeId};
use crate::value::{
    builtin_error_schema, make_index_error, make_key_error, make_parse_error, ClosureData,
    MapKey, OrderedMap, StructSchema, Value,
};

const MAX_CALL_DEPTH: usize = 256;
const INTERPRETER_STACK_SIZE: usize = 24 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}:{}] Runtime error: {}", self.span.line, self.span.col, self.message)
    }
}

impl std::error::Error for RuntimeError {}

enum ControlFlow {
    Next,
    Return(Value),
    Break,
    Continue,
}

enum InterpFlow {
    Return(Value),
    Error(RuntimeError),
}

impl From<RuntimeError> for InterpFlow {
    fn from(e: RuntimeError) -> Self {
        InterpFlow::Error(e)
    }
}

fn checked_float_to_int(value: f64, span: Span) -> Result<Value, RuntimeError> {
    let min = i64::MIN as f64;
    let max_exclusive = i64::MAX as f64;
    if !value.is_finite() || value < min || value >= max_exclusive {
        return Err(RuntimeError {
            message: "Cannot convert non-finite or out-of-range Float to Int".to_string(),
            span,
        });
    }
    Ok(Value::Int(value as i64))
}

type ModuleFuncMap = HashMap<(Option<PathBuf>, String), Arc<(Vec<Param>, Block)>>;
type JsonDecodePlanCache =
    RefCell<HashMap<Option<PathBuf>, HashMap<TypeExpr, Arc<JsonDecodePlan>>>>;

pub struct Interpreter {
    pub native_registry: NativeRegistry,
    module_functions: ModuleFuncMap,
    module_imports: HashMap<Option<PathBuf>, HashMap<String, ModuleTarget>>,
    module_structs: HashMap<Option<PathBuf>, HashSet<String>>,
    module_struct_type_params: HashMap<(Option<PathBuf>, String), Vec<String>>,
    module_struct_fields: HashMap<(Option<PathBuf>, String), Vec<crate::ast::FieldDef>>,
    module_error_fields: HashMap<(Option<PathBuf>, String), Vec<crate::ast::FieldDef>>,
    struct_schemas: RefCell<HashMap<TypeId, Arc<StructSchema>>>,
    json_decode_plans: JsonDecodePlanCache,
    module_errors: HashMap<Option<PathBuf>, HashSet<String>>,
    current_module: Option<PathBuf>,
    env: Vec<HashMap<String, Value>>,
    captured_env: Option<Arc<HashMap<String, Value>>>,
    call_depth: usize,
    large_stack_active: bool,
}

impl Interpreter {
    pub fn new() -> Self {
        Self {
            native_registry: NativeRegistry::new(),
            module_functions: HashMap::new(),
            module_imports: HashMap::new(),
            module_structs: HashMap::new(),
            module_struct_type_params: HashMap::new(),
            module_struct_fields: HashMap::new(),
            module_error_fields: HashMap::new(),
            struct_schemas: RefCell::new(HashMap::new()),
            json_decode_plans: RefCell::new(HashMap::new()),
            module_errors: HashMap::new(),
            current_module: None,
            env: vec![HashMap::new()],
            captured_env: None,
            call_depth: 0,
            large_stack_active: false,
        }
    }

    pub fn run_project(&mut self, root_path: &Path, loader: &ModuleLoader) -> Result<(), RuntimeError> {
        self.struct_schemas.get_mut().clear();
        self.json_decode_plans.get_mut().clear();
        self.module_error_fields.clear();
        for (path, loaded_mod) in &loader.modules {
            let mut imports_map = HashMap::new();
            for (alias, (target, _)) in &loaded_mod.imports {
                imports_map.insert(alias.clone(), target.clone());
            }
            self.module_imports.insert(Some(path.clone()), imports_map);

            let mut structs_set = HashSet::new();
            let mut errors_set = HashSet::new();

            for item in &loaded_mod.program.items {
                match &item.kind {
                    ItemKind::Func { name, params, body, .. } => {
                        self.module_functions.insert(
                            (Some(path.clone()), name.clone()),
                            Arc::new((params.clone(), body.clone())),
                        );
                    }
                    ItemKind::Type { name, type_params, fields } => {
                        structs_set.insert(name.clone());
                        self.module_struct_type_params
                            .insert((Some(path.clone()), name.clone()), type_params.clone());
                        self.module_struct_fields
                            .insert((Some(path.clone()), name.clone()), fields.clone());
                    }
                    ItemKind::Error { name, fields } => {
                        errors_set.insert(name.clone());
                        self.module_error_fields
                            .insert((Some(path.clone()), name.clone()), fields.clone());
                    }
                    ItemKind::Import { .. } => {}
                }
            }

            self.module_structs.insert(Some(path.clone()), structs_set);
            self.module_errors.insert(Some(path.clone()), errors_set);
        }

        let root_canon = root_path.to_path_buf();
        if !self.module_functions.contains_key(&(Some(root_canon.clone()), "main".to_string())) {
            return Err(RuntimeError {
                message: "Entry point 'func main()' not found in root module".to_string(),
                span: Span::new(1, 1),
            });
        }

        self.call_module_function(Some(root_canon), "main", vec![], Span::new(1, 1))?;
        Ok(())
    }

    pub fn run_program(&mut self, program: &Program) -> Result<(), RuntimeError> {
        self.struct_schemas.get_mut().clear();
        self.json_decode_plans.get_mut().clear();
        self.module_error_fields.clear();
        let mut structs_set = HashSet::new();
        let mut errors_set = HashSet::new();
        let mut imports_map = HashMap::new();

        for item in &program.items {
            match &item.kind {
                ItemKind::Func { name, params, body, .. } => {
                    self.module_functions.insert(
                        (None, name.clone()),
                        Arc::new((params.clone(), body.clone())),
                    );
                }
                ItemKind::Type { name, type_params, fields } => {
                    structs_set.insert(name.clone());
                    self.module_struct_type_params
                        .insert((None, name.clone()), type_params.clone());
                    self.module_struct_fields
                        .insert((None, name.clone()), fields.clone());
                }
                ItemKind::Error { name, fields } => {
                    errors_set.insert(name.clone());
                    self.module_error_fields
                        .insert((None, name.clone()), fields.clone());
                }
                ItemKind::Import { path, alias } => {
                    if path.starts_with("cdt.") {
                        imports_map.insert(alias.clone(), ModuleTarget::Native(path.clone()));
                    } else {
                        imports_map.insert(alias.clone(), ModuleTarget::Local(PathBuf::from(path)));
                    }
                }
            }
        }

        self.module_structs.insert(None, structs_set);
        self.module_errors.insert(None, errors_set);
        self.module_imports.insert(None, imports_map);

        if !self.module_functions.contains_key(&(None, "main".to_string())) {
            return Err(RuntimeError {
                message: "Entry point 'func main()' not found".to_string(),
                span: Span::new(1, 1),
            });
        }

        self.call_module_function(None, "main", vec![], Span::new(1, 1))?;
        Ok(())
    }

}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}
