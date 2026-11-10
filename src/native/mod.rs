use std::collections::HashMap;
use std::sync::Arc;

use crate::types::Type;
use crate::value::Value;

pub mod time;

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
