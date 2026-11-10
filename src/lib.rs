pub mod ast;
pub mod check;
pub mod interp;
pub mod lexer;
pub mod loader;
pub mod lsp;
pub mod native;
pub mod parser;
pub mod reachability;
pub mod types;
pub mod value;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
