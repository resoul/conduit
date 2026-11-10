use std::collections::HashMap;
use std::sync::Arc;

use crate::types::Type;
use crate::value::Value;

pub mod time;
pub mod validation;

pub trait NativeValue: std::fmt::Debug + Send + Sync {
    fn type_name(&self) -> &'static str;
    fn display(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result;
    fn as_any(&self) -> &dyn std::any::Any;
    fn dyn_eq(&self, other: &dyn NativeValue) -> bool;
}

pub type NativeFuncImpl = fn(args: Vec<Value>) -> Result<Value, Value>;
pub type NativeMethodImpl = fn(receiver: &Arc<dyn NativeValue>, args: Vec<Value>) -> Result<Value, Value>;

#[derive(Clone)]
pub struct NativeTypeDef {
    pub name: &'static str,
    pub qualified_name: &'static str,
    pub doc: &'static str,
    pub is_error: bool,
    pub fields: Vec<(&'static str, Type)>,
}

#[derive(Clone)]
pub struct NativeFuncDef {
    pub name: &'static str,
    pub doc: &'static str,
    pub params: Vec<(&'static str, Type)>,
    pub return_type: Type,
    pub is_validator: bool,
    pub implementation: NativeFuncImpl,
}

#[derive(Clone)]
pub struct NativeMethodDef {
    pub name: &'static str,
    pub doc: &'static str,
    pub params: Vec<(&'static str, Type)>,
    pub return_type: Type,
    pub implementation: NativeMethodImpl,
}

#[derive(Clone)]
pub struct NativeModule {
    pub name: &'static str,
    pub default_alias: &'static str,
    pub doc: &'static str,
    pub types: HashMap<&'static str, NativeTypeDef>,
    pub functions: HashMap<&'static str, NativeFuncDef>,
    pub methods: HashMap<(&'static str, &'static str), NativeMethodDef>,
}

impl NativeModule {
    pub fn new(name: &'static str, default_alias: &'static str, doc: &'static str) -> Self {
        Self {
            name,
            default_alias,
            doc,
            types: HashMap::new(),
            functions: HashMap::new(),
            methods: HashMap::new(),
        }
    }

    pub fn add_type(&mut self, type_def: NativeTypeDef) {
        self.types.insert(type_def.name, type_def);
    }

    pub fn add_function(&mut self, func_def: NativeFuncDef) {
        self.functions.insert(func_def.name, func_def);
    }

    pub fn add_method(&mut self, target_type: &'static str, method_def: NativeMethodDef) {
        self.methods.insert((target_type, method_def.name), method_def);
    }

    pub fn generate_markdown_docs(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("# {}\n\n", self.name));
        out.push_str(&format!("{}\n\n", self.doc));
        out.push_str("```conduit\n");
        out.push_str(&format!("import {} as {}\n", self.name, self.default_alias));
        out.push_str("```\n\n");

        out.push_str("## Types\n\n");
        let mut sorted_types: Vec<_> = self.types.values().collect();
        sorted_types.sort_by_key(|t| t.name);
        for t in sorted_types {
            let cat = if t.is_error {
                "Error Type"
            } else {
                "Opaque Type"
            };
            out.push_str(&format!("### {}\n\n", t.name));
            out.push_str(&format!("- **Qualified Name:** `{}`\n", t.qualified_name));
            out.push_str(&format!("- **Category:** {}\n", cat));
            out.push_str(&format!("- **Description:** {}\n", t.doc));
            if !t.fields.is_empty() {
                out.push_str("- **Fields:**\n");
                for (fname, fty) in &t.fields {
                    out.push_str(&format!("  - `{}: {}`\n", fname, fty));
                }
            }
            out.push('\n');
        }

        out.push_str("## Functions\n\n");
        let mut sorted_funcs: Vec<_> = self.functions.values().collect();
        sorted_funcs.sort_by_key(|f| f.name);
        out.push_str("| Function | Signature | Description |\n");
        out.push_str("| :--- | :--- | :--- |\n");
        for f in sorted_funcs {
            let params = f
                .params
                .iter()
                .map(|(pname, pty)| format!("{}: {}", pname, pty))
                .collect::<Vec<_>>()
                .join(", ");
            let sig = format!("{}({}) -> {}", f.name, params, f.return_type);
            let escaped_sig = sig.replace('|', "\\|");
            out.push_str(&format!(
                "| `{}` | `{}` | {} |\n",
                f.name, escaped_sig, f.doc
            ));
        }
        out.push('\n');

        out.push_str("## Methods\n\n");
        let mut sorted_methods: Vec<_> = self.methods.iter().collect();
        sorted_methods.sort_by_key(|((target, name), _)| (*target, *name));
        out.push_str("| Type | Method | Signature | Description |\n");
        out.push_str("| :--- | :--- | :--- | :--- |\n");
        for ((target, name), m) in sorted_methods {
            let params = m
                .params
                .iter()
                .map(|(pname, pty)| format!("{}: {}", pname, pty))
                .collect::<Vec<_>>()
                .join(", ");
            let sig = format!("{}({}) -> {}", name, params, m.return_type);
            let escaped_sig = sig.replace('|', "\\|");
            out.push_str(&format!(
                "| `{}` | `{}` | `{}` | {} |\n",
                target, name, escaped_sig, m.doc
            ));
        }

        out
    }
}

pub struct NativeRegistry {
    modules: HashMap<&'static str, NativeModule>,
}

impl NativeRegistry {
    pub fn new() -> Self {
        let mut reg = Self {
            modules: HashMap::new(),
        };
        reg.register_module(time::create_time_module());
        reg.register_module(validation::create_validation_module());
        reg
    }

    pub fn register_module(&mut self, module: NativeModule) {
        self.modules.insert(module.name, module);
    }

    pub fn get_module(&self, name: &str) -> Option<&NativeModule> {
        self.modules.get(name).or_else(|| {
            self.modules.values().find(|m| m.default_alias == name)
        })
    }

    pub fn modules(&self) -> impl Iterator<Item = &NativeModule> {
        self.modules.values()
    }
}

impl Default for NativeRegistry {
    fn default() -> Self {
        Self::new()
    }
}
