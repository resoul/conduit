use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::{ItemKind, Program};
use crate::lexer::{Lexer, Span};
use crate::native::NativeRegistry;
use crate::parser::Parser;

#[derive(Debug, Clone, PartialEq)]
pub struct LoaderError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for LoaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}:{}] Loader error: {}", self.span.line, self.span.col, self.message)
    }
}

impl std::error::Error for LoaderError {}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ModuleTarget {
    Native(String),
    Local(PathBuf),
}

#[derive(Debug, Clone)]
pub struct LoadedModule {
    pub canonical_path: PathBuf,
    pub program: Program,
    pub imports: HashMap<String, (ModuleTarget, Span)>, // alias -> (target, span)
}

pub struct ModuleLoader {
    native_registry: NativeRegistry,
    virtual_files: HashMap<PathBuf, String>,
    pub modules: HashMap<PathBuf, LoadedModule>,
}

impl ModuleLoader {
    pub fn new() -> Self {
        Self {
            native_registry: NativeRegistry::new(),
            virtual_files: HashMap::new(),
            modules: HashMap::new(),
        }
    }

    pub fn with_virtual_file(&mut self, path: impl AsRef<Path>, content: impl Into<String>) {
        let p = path.as_ref();
        let canon = if p.is_absolute() {
            p.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(p)
        };
        self.virtual_files.insert(canon, content.into());
    }

    pub fn load_project(&mut self, root_path: &Path) -> Result<PathBuf, LoaderError> {
        let root_canon = self.canonicalize_path(root_path, Span::new(1, 1))?;
        let mut load_stack = Vec::new();
        self.load_recursive(&root_canon, &mut load_stack)?;
        Ok(root_canon)
    }

    fn canonicalize_path(&self, path: &Path, span: Span) -> Result<PathBuf, LoaderError> {
        if self.virtual_files.contains_key(path) {
            return Ok(path.to_path_buf());
        }

        match fs::canonicalize(path) {
            Ok(p) => Ok(p),
            Err(err) => {
                // If path is in virtual_files after resolving against current dir
                let abs = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(path)
                };
                if self.virtual_files.contains_key(&abs) {
                    return Ok(abs);
                }
                Err(LoaderError {
                    message: format!("Cannot resolve path '{}': {}", path.display(), err),
                    span,
                })
            }
        }
    }

    fn read_content(&self, path: &Path, span: Span) -> Result<String, LoaderError> {
        if let Some(content) = self.virtual_files.get(path) {
            return Ok(content.clone());
        }

        fs::read_to_string(path).map_err(|err| LoaderError {
            message: format!("Cannot read module file '{}': {}", path.display(), err),
            span,
        })
    }

    fn load_recursive(
        &mut self,
        canonical_path: &Path,
        load_stack: &mut Vec<PathBuf>,
    ) -> Result<(), LoaderError> {
        if self.modules.contains_key(canonical_path) {
            return Ok(());
        }

        if let Some(pos) = load_stack.iter().position(|p| p == canonical_path) {
            let mut chain: Vec<String> = load_stack[pos..]
                .iter()
                .map(|p| p.file_name().unwrap_or_default().to_string_lossy().to_string())
                .collect();
            chain.push(canonical_path.file_name().unwrap_or_default().to_string_lossy().to_string());
            return Err(LoaderError {
                message: format!("Cyclic import detected: {}", chain.join(" -> ")),
                span: Span::new(1, 1),
            });
        }

        load_stack.push(canonical_path.to_path_buf());

        let source = self.read_content(canonical_path, Span::new(1, 1))?;
        let mut lexer = Lexer::new(&source);
        let tokens = lexer.tokenize_all().map_err(|err| LoaderError {
            message: err.message,
            span: err.span,
        })?;

        let mut parser = Parser::new(tokens);
        let program = parser.parse_program().map_err(|err| LoaderError {
            message: err.message,
            span: err.span,
        })?;

        let current_dir = canonical_path.parent().unwrap_or_else(|| Path::new("."));
        let mut imports_map: HashMap<String, (ModuleTarget, Span)> = HashMap::new();

        // Check for local top-level declaration names to detect alias conflicts
        let mut local_declared_names = HashMap::new();
        for item in &program.items {
            match &item.kind {
                ItemKind::Func { name, .. }
                | ItemKind::Type { name, .. }
                | ItemKind::Error { name, .. } => {
                    local_declared_names.insert(name.clone(), item.span);
                }
                _ => {}
            }
        }

        let mut pending_submodules = Vec::new();

        for item in &program.items {
            if let ItemKind::Import { path: ref import_path, ref alias } = item.kind {
                if let Some((_, prev_span)) = imports_map.get(alias) {
                    return Err(LoaderError {
                        message: format!(
                            "Alias conflict: alias '{}' is already imported at line {}",
                            alias, prev_span.line
                        ),
                        span: item.span,
                    });
                }

                if let Some(decl_span) = local_declared_names.get(alias) {
                    return Err(LoaderError {
                        message: format!(
                            "Alias conflict: import alias '{}' conflicts with top-level declaration at line {}",
                            alias, decl_span.line
                        ),
                        span: item.span,
                    });
                }

                if import_path.starts_with("cdt.") {
                    if self.native_registry.get_module(import_path).is_none() {
                        return Err(LoaderError {
                            message: format!("Unknown standard library module '{}'", import_path),
                            span: item.span,
                        });
                    }
                    imports_map.insert(
                        alias.clone(),
                        (ModuleTarget::Native(import_path.clone()), item.span),
                    );
                } else if import_path.starts_with("std.") {
                    return Err(LoaderError {
                        message: format!(
                            "Standard library uses 'cdt.*' prefix, 'std.*' is not supported: '{}'",
                            import_path
                        ),
                        span: item.span,
                    });
                } else {
                    // Local module import
                    let relative_file = format!("{}.cdt", import_path);
                    let target_raw = current_dir.join(relative_file);
                    let target_canon = self.canonicalize_path(&target_raw, item.span)?;

                    imports_map.insert(
                        alias.clone(),
                        (ModuleTarget::Local(target_canon.clone()), item.span),
                    );
                    pending_submodules.push(target_canon);
                }
            }
        }

        for sub in pending_submodules {
            self.load_recursive(&sub, load_stack)?;
        }

        load_stack.pop();

        self.modules.insert(
            canonical_path.to_path_buf(),
            LoadedModule {
                canonical_path: canonical_path.to_path_buf(),
                program,
                imports: imports_map,
            },
        );

        Ok(())
    }
}

impl Default for ModuleLoader {
    fn default() -> Self {
        Self::new()
    }
}
