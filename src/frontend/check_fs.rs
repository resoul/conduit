use crate::frontend::ast::{Expr, NamedArg, TypeExpr};
use crate::frontend::check::Checker;
use crate::frontend::check_native::check_fields_ctor;
use crate::frontend::types::{Type, TypeId};

impl Checker<'_> {
    pub(crate) fn check_fs_call(
        &mut self,
        sym: &str,
        type_args: &[TypeExpr],
        args: &mut [Expr],
        named_args: &mut [NamedArg],
    ) -> Result<Type, String> {
        let fs_err = Type::Custom(TypeId::native("cdt.fs", "FsError"));
        let reader_ty = Type::Custom(TypeId::native("cdt.fs", "Reader"));
        let writer_ty = Type::Custom(TypeId::native("cdt.fs", "Writer"));

        match sym {
            "FsError" => {
                if !type_args.is_empty() {
                    return Err("FsError does not accept type arguments".to_string());
                }
                check_fields_ctor(
                    self,
                    "FsError",
                    &[
                        ("kind", Type::String),
                        ("path", Type::String),
                        ("message", Type::String),
                    ],
                    args,
                    named_args,
                    fs_err,
                )
            }
            "read_text" | "read_lines" | "exists" | "is_file" | "is_dir" | "size" | "list_dir"
            | "mkdir_all" | "remove" | "open_read" | "open_write" | "open_append" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 1 {
                    return Err(format!("fs.{}(path) expects 1 String argument", sym));
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!("fs.{} expects String, got {}", sym, args[0].ty));
                }
                match sym {
                    "read_text" => Ok(Type::union(vec![Type::String, fs_err])),
                    "read_lines" | "list_dir" => {
                        Ok(Type::union(vec![Type::List(Box::new(Type::String)), fs_err]))
                    }
                    "exists" | "is_file" | "is_dir" => Ok(Type::Bool),
                    "size" => Ok(Type::union(vec![Type::Int, fs_err])),
                    "mkdir_all" | "remove" => Ok(Type::union(vec![Type::Void, fs_err])),
                    "open_read" => Ok(Type::union(vec![reader_ty, fs_err])),
                    "open_write" | "open_append" => Ok(Type::union(vec![writer_ty, fs_err])),
                    _ => unreachable!(),
                }
            }
            "write_text" | "append_text" | "write_atomic" | "copy" | "rename" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 2 {
                    return Err(format!("fs.{} expects 2 String arguments", sym));
                }
                let (a0, a1) = args.split_at_mut(1);
                self.check_expr(&mut a0[0], Some(&Type::String))?;
                if a0[0].ty != Type::String {
                    return Err(format!("fs.{} expects String as arg 1, got {}", sym, a0[0].ty));
                }
                self.check_expr(&mut a1[0], Some(&Type::String))?;
                if a1[0].ty != Type::String {
                    return Err(format!("fs.{} expects String as arg 2, got {}", sym, a1[0].ty));
                }
                Ok(Type::union(vec![Type::Void, fs_err]))
            }
            "each_line" => {
                if !type_args.is_empty() || !named_args.is_empty() || args.len() != 2 {
                    return Err("fs.each_line(path, cb) expects 2 arguments".to_string());
                }
                let (path_slice, cb_slice) = args.split_at_mut(1);
                self.check_expr(&mut path_slice[0], Some(&Type::String))?;
                if path_slice[0].ty != Type::String {
                    return Err(format!(
                        "fs.each_line expects String path, got {}",
                        path_slice[0].ty
                    ));
                }
                let expected_cb = Type::Function {
                    params: vec![Type::String],
                    return_type: Box::new(Type::Void),
                };
                self.check_expr(&mut cb_slice[0], Some(&expected_cb))?;
                let Type::Function {
                    params,
                    return_type,
                } = cb_slice[0].ty.clone()
                else {
                    return Err(format!(
                        "fs.each_line callback must be (String) -> Void, got {}",
                        cb_slice[0].ty
                    ));
                };
                if params.len() != 1 || params[0] != Type::String || *return_type != Type::Void {
                    return Err(format!(
                        "fs.each_line callback must be (String) -> Void, got {}",
                        cb_slice[0].ty
                    ));
                }
                Ok(Type::union(vec![Type::Int, fs_err]))
            }
            other => Err(format!("Unknown cdt.fs symbol '{}'", other)),
        }
    }

    pub(crate) fn check_fs_method(
        &mut self,
        type_name: &str,
        method: &str,
        args: &mut [Expr],
    ) -> Result<Type, String> {
        let fs_err = Type::Custom(TypeId::native("cdt.fs", "FsError"));
        match (type_name, method) {
            ("Reader", "read_line") => {
                if !args.is_empty() {
                    return Err("Reader.read_line() takes 0 arguments".to_string());
                }
                Ok(Type::union(vec![Type::String, Type::Nil, fs_err]))
            }
            ("Reader", "read_chunk") => {
                if args.len() != 1 {
                    return Err("Reader.read_chunk(max_bytes) expects 1 Int argument".to_string());
                }
                self.check_expr(&mut args[0], Some(&Type::Int))?;
                if args[0].ty != Type::Int {
                    return Err(format!(
                        "Reader.read_chunk expects Int, got {}",
                        args[0].ty
                    ));
                }
                Ok(Type::union(vec![Type::String, Type::Nil, fs_err]))
            }
            ("Reader", "close") | ("Writer", "close") => {
                if !args.is_empty() {
                    return Err(format!("{}.close() takes 0 arguments", type_name));
                }
                Ok(Type::Void)
            }
            ("Writer", "write" | "write_line") => {
                if args.len() != 1 {
                    return Err(format!(
                        "Writer.{}(data) expects 1 String argument",
                        method
                    ));
                }
                self.check_expr(&mut args[0], Some(&Type::String))?;
                if args[0].ty != Type::String {
                    return Err(format!(
                        "Writer.{} expects String, got {}",
                        method, args[0].ty
                    ));
                }
                Ok(Type::union(vec![Type::Void, fs_err]))
            }
            ("Writer", "write_lines") => {
                if args.len() != 1 {
                    return Err("Writer.write_lines(lines) expects 1 [String] argument".to_string());
                }
                let expected = Type::List(Box::new(Type::String));
                self.check_expr(&mut args[0], Some(&expected))?;
                if args[0].ty != expected {
                    return Err(format!(
                        "Writer.write_lines expects [String], got {}",
                        args[0].ty
                    ));
                }
                Ok(Type::union(vec![Type::Void, fs_err]))
            }
            ("Writer", "flush") => {
                if !args.is_empty() {
                    return Err("Writer.flush() takes 0 arguments".to_string());
                }
                Ok(Type::union(vec![Type::Void, fs_err]))
            }
            _ => Err(format!("Type '{}' has no method '.{}()'", type_name, method)),
        }
    }
}
