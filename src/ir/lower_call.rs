use std::collections::HashSet;

use crate::frontend::ast::*;
use crate::frontend::check::type_to_json_type;
use crate::frontend::check_native::scalar_kind;
use crate::frontend::types::{ModuleId, Type};
use crate::ir::lower::FnLowerer;
use crate::ir::lower_loops::{collect_free_vars, patch_jump};
use crate::ir::lower_native::elem_tag_for;
use crate::ir::{CallableRef, ElemKind, PipelinePlan, PipelineStage, RecordDesc};
use crate::vm::opcode::{
    EXT_FLOAT_CEIL, EXT_FLOAT_MATH, EXT_INT_MATH, EXT_LIST_IS_EMPTY, EXT_MAP_CONTAINS_KEY,
    EXT_MAP_FROM, EXT_MAP_SET, EXT_PRINT_OBJ, EXT_PRINT_SCALAR, EXT_STR_IS_EMPTY, EXT_STR_PRED,
    Op,
};

impl<'a, 'b> FnLowerer<'a, 'b> {
    pub(crate) fn lower_closure(
        &mut self,
        params: &[Param],
        return_type: &Option<TypeExpr>,
        body: &Block,
        closure_ty: &Type,
    ) -> Result<u16, String> {
        let mut free = HashSet::new();
        collect_free_vars(body, params, &mut free);
        let mut free_sorted: Vec<String> = free.into_iter().collect();
        free_sorted.sort();

        let mut cap_s = Vec::new();
        let mut cap_o = Vec::new();
        for name in free_sorted {
            if let Some((slot, ty)) = self.lookup_local(&name) {
                if slot.is_scalar {
                    cap_s.push((name, ty, slot.reg));
                } else {
                    cap_o.push((name, ty, slot.reg));
                }
            }
        }

        let (param_tys, ret_ty) = match closure_ty {
            Type::Function { params: pt, return_type: rt } => (pt.clone(), (**rt).clone()),
            _ => {
                let mut pt = Vec::with_capacity(params.len());
                for p in params {
                    pt.push(self.resolve_type(&p.ty)?);
                }
                let rt = match return_type {
                    Some(r) => self.resolve_type(r)?,
                    None => Type::Void,
                };
                (pt, rt)
            }
        };

        let proto_idx = self.mc.compile_closure(params, &param_tys, &ret_ty, body, &cap_s, &cap_o)?;
        let dst_obj = self.alloc_temp_o();
        self.ops.push(Op::MakeClosure { dst_obj, proto_idx });
        Ok(dst_obj)
    }

    pub(crate) fn lower_void_call(&mut self, expr: &Expr) -> Result<(), String> {
        let ExprKind::Call { callee, args, named_args, .. } = &expr.kind else {
            return Err(format!("Unsupported void expression {:?}", expr.kind));
        };
        if !named_args.is_empty() {
            return Err("Unexpected named arguments in void call".to_string());
        }
        if let ExprKind::Member { target, field } = &callee.kind {
            if let ExprKind::Ident(alias) = &target.kind
                && self.lookup_local(alias).is_none()
                && self.mc.checked.process_aliases.contains(alias)
            {
                let _ = self.lower_process_call(field, args, named_args)?;
                return Ok(());
            }
            return self.lower_void_method(target, field, args);
        }
        if let ExprKind::Ident(name) = &callee.kind
            && self.lookup_local(name).is_none()
            && let Some(sym) = self.mc.checked.process_direct_fns.get(name)
        {
            let sym = sym.clone();
            let _ = self.lower_process_call(&sym, args, named_args)?;
            return Ok(());
        }
        if let ExprKind::Ident(name) = &callee.kind
            && name == "print"
            && self.lookup_local(name).is_none()
        {
            if args[0].ty.is_scalar() {
                let src = self.lower_scalar(&args[0])?;
                let aux = elem_tag_for(&args[0].ty);
                self.ops.push(Op::ExtOp { kind: EXT_PRINT_SCALAR, aux, a: src, b: 0, c: 0 });
            } else {
                let src = self.lower_obj(&args[0])?;
                self.ops.push(Op::ExtOp { kind: EXT_PRINT_OBJ, aux: 0, a: src, b: 0, c: 0 });
            }
            return Ok(());
        }
        if let Type::Function { return_type, .. } = &callee.ty
            && (return_type.is_scalar() || return_type.is_obj())
        {
            let _ = self.lower_func_call(callee, args, return_type.is_scalar(), return_type.is_obj())?;
            return Ok(());
        }
        let _ = self.lower_func_call(callee, args, false, false)?;
        Ok(())
    }

    pub(crate) fn lower_call_scalar(&mut self, expr: &Expr) -> Result<u16, String> {
        let ExprKind::Call { callee, args, named_args, .. } = &expr.kind else {
            return Err("Expected call".to_string());
        };
        if !named_args.is_empty() {
            return Err("Unexpected named arguments in scalar call".to_string());
        }
        if let ExprKind::Member { target, field } = &callee.kind {
            if let ExprKind::Ident(alias) = &target.kind
                && self.lookup_local(alias).is_none()
            {
                if self.mc.checked.fs_aliases.contains(alias) {
                    return self.lower_fs_call(field, args, named_args);
                }
                if self.mc.checked.path_aliases.contains(alias) {
                    return self.lower_path_call(field, args);
                }
                if self.mc.checked.process_aliases.contains(alias) {
                    return self.lower_process_call(field, args, named_args);
                }
                if self.mc.checked.crypto_aliases.contains(alias) {
                    return self.lower_crypto_call(field, args, named_args);
                }
                if self.mc.checked.path_aliases.contains(alias) {
                    return self.lower_path_call(field, args);
                }
                if self.mc.checked.process_aliases.contains(alias) {
                    return self.lower_process_call(field, args, named_args);
                }
            }
            return self.lower_scalar_method(target, field, args);
        }
        if let ExprKind::Ident(name) = &callee.kind
            && self.lookup_local(name).is_none()
        {
            if let Some(sym) = self.mc.checked.fs_direct_fns.get(name) {
                let sym = sym.clone();
                return self.lower_fs_call(&sym, args, named_args);
            }
            if let Some(sym) = self.mc.checked.path_direct_fns.get(name) {
                let sym = sym.clone();
                return self.lower_path_call(&sym, args);
            }
            if let Some(sym) = self.mc.checked.process_direct_fns.get(name) {
                let sym = sym.clone();
                return self.lower_process_call(&sym, args, named_args);
            }
        }
        if let ExprKind::Ident(fn_name) = &callee.kind
            && self.lookup_local(fn_name).is_none()
            && fn_name == "reduce"
        {
            return self.lower_reduce_call(&args[0], &args[1], &args[2], &expr.ty);
        }
        self.lower_func_call(callee, args, true, false)
    }

    pub(crate) fn lower_call_obj(&mut self, expr: &Expr) -> Result<u16, String> {
        let ExprKind::Call { callee, type_args, args, named_args } = &expr.kind else {
            return Err("Expected call".to_string());
        };

        if let ExprKind::Ident(fn_name) = &callee.kind
            && self.lookup_local(fn_name).is_none()
            && (fn_name == "map" || fn_name == "filter")
        {
            return self.lower_pipeline_call(expr);
        }
        if let ExprKind::Ident(fn_name) = &callee.kind
            && self.lookup_local(fn_name).is_none()
            && fn_name == "reduce"
        {
            return self.lower_reduce_call(&args[0], &args[1], &args[2], &expr.ty);
        }

        if let ExprKind::Member { target, field } = &callee.kind {
            if let ExprKind::Ident(alias) = &target.kind
                && self.lookup_local(alias).is_none()
            {
                if alias == "Map" && field == "from" {
                    let (Type::List(inner), _) = (&args[0].ty, ()) else {
                        return Err("Map.from() expects list of pairs".to_string());
                    };
                    let Type::Tuple(elems) = inner.as_ref() else {
                        return Err("Map.from() expects list of pairs".to_string());
                    };
                    let list_obj = self.lower_obj(&args[0])?;
                    let aux = (elem_tag_for(&elems[0]) << 4) | elem_tag_for(&elems[1]);
                    let dst_obj = self.alloc_temp_o();
                    self.ops.push(Op::ExtOp {
                        kind: EXT_MAP_FROM,
                        aux,
                        a: dst_obj,
                        b: list_obj,
                        c: 0,
                    });
                    return Ok(dst_obj);
                }
                if self.mc.checked.json_aliases.contains(alias) {
                    return self.lower_json_call(field, type_args, args);
                }
                if self.mc.checked.time_aliases.contains(alias) {
                    return self.lower_time_call(field, args, named_args);
                }
                if self.mc.checked.val_aliases.contains(alias) {
                    return self.lower_validation_call(field, args, named_args);
                }
                if self.mc.checked.env_aliases.contains(alias) {
                    return self.lower_env_call(field, type_args, args, named_args);
                }
                if self.mc.checked.cli_aliases.contains(alias) {
                    return self.lower_cli_call(field, type_args, args, named_args);
                }
                if self.mc.checked.fs_aliases.contains(alias) {
                    return self.lower_fs_call(field, args, named_args);
                }
                if self.mc.checked.path_aliases.contains(alias) {
                    return self.lower_path_call(field, args);
                }
                if self.mc.checked.process_aliases.contains(alias) {
                    return self.lower_process_call(field, args, named_args);
                }
                if self.mc.checked.crypto_aliases.contains(alias) {
                    return self.lower_crypto_call(field, args, named_args);
                }
            }
            if let Some(reg) = self.lower_builtin_obj_method(target, field, args)? {
                return Ok(reg);
            }
            if let Type::Custom(tid) = &target.ty
                && tid.module != ModuleId::Program
            {
                return self.lower_native_method_obj(tid, target, field, args);
            }
        }

        if let ExprKind::Ident(name) = &callee.kind
            && self.lookup_local(name).is_none()
        {
            if let Some(method) = self.mc.checked.json_direct_fns.get(name) {
                let method = method.clone();
                return self.lower_json_call(&method, type_args, args);
            }
            if let Some(sym) = self.mc.checked.time_direct_fns.get(name) {
                let sym = sym.clone();
                return self.lower_time_call(&sym, args, named_args);
            }
            if let Some(sym) = self.mc.checked.time_direct_types.get(name) {
                let sym = sym.clone();
                return self.lower_time_call(&sym, args, named_args);
            }
            if let Some(sym) = self.mc.checked.val_direct_fns.get(name) {
                let sym = sym.clone();
                return self.lower_validation_call(&sym, args, named_args);
            }
            if let Some(sym) = self.mc.checked.val_direct_types.get(name) {
                let sym = sym.clone();
                return self.lower_validation_call(&sym, args, named_args);
            }
            if let Some(sym) = self.mc.checked.env_direct_fns.get(name) {
                let sym = sym.clone();
                return self.lower_env_call(&sym, type_args, args, named_args);
            }
            if let Some(sym) = self.mc.checked.env_direct_types.get(name) {
                let sym = sym.clone();
                return self.lower_env_call(&sym, type_args, args, named_args);
            }
            if let Some(sym) = self.mc.checked.cli_direct_fns.get(name) {
                let sym = sym.clone();
                return self.lower_cli_call(&sym, type_args, args, named_args);
            }
            if let Some(sym) = self.mc.checked.cli_direct_types.get(name) {
                let sym = sym.clone();
                return self.lower_cli_call(&sym, type_args, args, named_args);
            }
            if let Some(sym) = self.mc.checked.fs_direct_fns.get(name) {
                let sym = sym.clone();
                return self.lower_fs_call(&sym, args, named_args);
            }
            if let Some(sym) = self.mc.checked.fs_direct_types.get(name) {
                let sym = sym.clone();
                return self.lower_fs_call(&sym, args, named_args);
            }
            if let Some(sym) = self.mc.checked.path_direct_fns.get(name) {
                let sym = sym.clone();
                return self.lower_path_call(&sym, args);
            }
            if let Some(sym) = self.mc.checked.process_direct_fns.get(name) {
                let sym = sym.clone();
                return self.lower_process_call(&sym, args, named_args);
            }
            if let Some(sym) = self.mc.checked.process_direct_types.get(name) {
                let sym = sym.clone();
                return self.lower_process_call(&sym, args, named_args);
            }
            if let Some(sym) = self.mc.checked.crypto_direct_fns.get(name) {
                let sym = sym.clone();
                return self.lower_crypto_call(&sym, args, named_args);
            }
            if let Some(sym) = self.mc.checked.crypto_direct_types.get(name) {
                let sym = sym.clone();
                return self.lower_crypto_call(&sym, args, named_args);
            }
            if name == "Map" {
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::NewMap { dst_obj });
                return Ok(dst_obj);
            }
            if matches!(name.as_str(), "IndexError" | "KeyError" | "ParseError") {
                return self.lower_prelude_error_ctor(name, args, named_args);
            }
            if self.mc.checked.struct_by_name.contains_key(name) {
                return self.lower_struct_ctor(name, args, named_args);
            }
        }

        self.lower_func_call(callee, args, false, true)
    }

    fn lower_void_method(&mut self, target: &Expr, method: &str, args: &[Expr]) -> Result<(), String> {
        match (&target.ty, method) {
            (Type::List(elem), "append") => {
                let list_obj = if let ExprKind::Ident(name) = &target.kind
                    && let Some((slot, _)) = self.lookup_local(name)
                {
                    slot.reg
                } else {
                    self.lower_obj(target)?
                };
                if elem.is_int() || elem.is_bool() {
                    let val_s = self.lower_scalar(&args[0])?;
                    self.ops.push(Op::ListAppendInt { list_obj, val_s });
                } else if elem.is_float() {
                    let val_s = self.lower_scalar(&args[0])?;
                    self.ops.push(Op::ListAppendFloat { list_obj, val_s });
                } else {
                    let val_obj = self.lower_obj(&args[0])?;
                    self.ops.push(Op::ListAppendObj { list_obj, val_obj });
                }
                Ok(())
            }
            (Type::Map(k, v), "set") => {
                let map_obj = if let ExprKind::Ident(name) = &target.kind
                    && let Some((slot, _)) = self.lookup_local(name)
                {
                    slot.reg
                } else {
                    self.lower_obj(target)?
                };
                if **k == Type::Int && **v == Type::Int {
                    let key_s = self.lower_scalar(&args[0])?;
                    let val_s = self.lower_scalar(&args[1])?;
                    self.ops.push(Op::MapSetIntInt { map_obj, key_s, val_s });
                    Ok(())
                } else {
                    let key_reg = if k.is_scalar() {
                        self.lower_scalar(&args[0])?
                    } else {
                        self.lower_obj(&args[0])?
                    };
                    let val_reg = if v.is_scalar() {
                        self.lower_scalar(&args[1])?
                    } else {
                        self.lower_obj(&args[1])?
                    };
                    let aux = (elem_tag_for(k) << 4) | elem_tag_for(v);
                    self.ops.push(Op::ExtOp {
                        kind: EXT_MAP_SET,
                        aux,
                        a: map_obj,
                        b: key_reg,
                        c: val_reg,
                    });
                    Ok(())
                }
            }
            (Type::Custom(tid), _) if matches!(&tid.module, ModuleId::Native(m) if m == "cdt.fs") => {
                self.lower_native_method_obj(tid, target, method, args)?;
                Ok(())
            }
            _ => Err(format!("Unsupported void method '.{}()'", method)),
        }
    }

    fn lower_scalar_method(
        &mut self,
        target: &Expr,
        method: &str,
        args: &[Expr],
    ) -> Result<u16, String> {
        match (&target.ty, method) {
            (Type::List(_), "len") => {
                let list_obj = self.lower_obj(target)?;
                let dst_s = self.alloc_temp_s();
                self.ops.push(Op::ListLen { dst_s, list_obj });
                Ok(dst_s)
            }
            (Type::List(_), "is_empty") => {
                let list_obj = self.lower_obj(target)?;
                let dst_s = self.alloc_temp_s();
                self.ops.push(Op::ExtOp {
                    kind: EXT_LIST_IS_EMPTY,
                    aux: 0,
                    a: dst_s,
                    b: list_obj,
                    c: 0,
                });
                Ok(dst_s)
            }
            (Type::List(_), "any") => self.lower_list_pred_loop(target, &args[0], 0),
            (Type::List(_), "all") => self.lower_list_pred_loop(target, &args[0], 1),
            (Type::List(elem), "contains") => {
                let list_obj = self.lower_obj(target)?;
                let dst_s = self.alloc_temp_s();
                if elem.is_scalar() {
                    let val_s = self.lower_scalar(&args[0])?;
                    let elem_tag = crate::ir::lower_native::elem_tag_for(elem);
                    self.ops.push(Op::ListContainsScalar { dst_s, list_obj, val_s, elem_tag });
                } else {
                    let val_obj = self.lower_obj(&args[0])?;
                    self.ops.push(Op::ListContainsObj { dst_s, list_obj, val_obj });
                }
                Ok(dst_s)
            }
            (Type::String, "len") => {
                let str_obj = self.lower_obj(target)?;
                let dst_s = self.alloc_temp_s();
                self.ops.push(Op::StrLen { dst_s, str_obj });
                Ok(dst_s)
            }
            (Type::String, "is_empty") => {
                let str_obj = self.lower_obj(target)?;
                let dst_s = self.alloc_temp_s();
                self.ops.push(Op::ExtOp {
                    kind: EXT_STR_IS_EMPTY,
                    aux: 0,
                    a: dst_s,
                    b: str_obj,
                    c: 0,
                });
                Ok(dst_s)
            }
            (Type::String, "contains" | "starts_with" | "ends_with") => {
                let str_obj = self.lower_obj(target)?;
                let arg_obj = self.lower_obj(&args[0])?;
                let dst_s = self.alloc_temp_s();
                let aux = match method {
                    "contains" => 0,
                    "starts_with" => 1,
                    _ => 2,
                };
                self.ops.push(Op::ExtOp {
                    kind: EXT_STR_PRED,
                    aux,
                    a: dst_s,
                    b: str_obj,
                    c: arg_obj,
                });
                Ok(dst_s)
            }
            (Type::Map(_, _), "len") => {
                let map_obj = self.lower_obj(target)?;
                let dst_s = self.alloc_temp_s();
                self.ops.push(Op::MapLen { dst_s, map_obj });
                Ok(dst_s)
            }
            (Type::Int, "to_float") => {
                let src = self.lower_scalar(target)?;
                let dst = self.alloc_temp_s();
                self.ops.push(Op::IntToFloat { dst, src });
                Ok(dst)
            }
            (Type::Float, "floor") => {
                let src = self.lower_scalar(target)?;
                let dst = self.alloc_temp_s();
                self.ops.push(Op::FloatFloor { dst, src });
                Ok(dst)
            }
            (Type::Float, "round") => {
                let src = self.lower_scalar(target)?;
                let dst = self.alloc_temp_s();
                self.ops.push(Op::FloatRound { dst, src });
                Ok(dst)
            }
            (Type::Float, "ceil") => {
                let src = self.lower_scalar(target)?;
                let dst = self.alloc_temp_s();
                self.ops.push(Op::ExtOp { kind: EXT_FLOAT_CEIL, aux: 0, a: dst, b: src, c: 0 });
                Ok(dst)
            }
            (Type::Int, "abs") | (Type::Float, "abs") => {
                let src = self.lower_scalar(target)?;
                let dst = self.alloc_temp_s();
                let kind = if target.ty == Type::Int { EXT_INT_MATH } else { EXT_FLOAT_MATH };
                self.ops.push(Op::ExtOp { kind, aux: 0, a: dst, b: src, c: 0 });
                Ok(dst)
            }
            (Type::Int, "min" | "max") | (Type::Float, "min" | "max") => {
                let src = self.lower_scalar(target)?;
                let other = self.lower_scalar(&args[0])?;
                let dst = self.alloc_temp_s();
                let kind = if target.ty == Type::Int { EXT_INT_MATH } else { EXT_FLOAT_MATH };
                let aux = if method == "min" { 1 } else { 2 };
                self.ops.push(Op::ExtOp { kind, aux, a: dst, b: src, c: other });
                Ok(dst)
            }
            (Type::Int, "clamp") | (Type::Float, "clamp") => {
                let val_s = self.lower_scalar(target)?;
                let lo_s = self.lower_scalar(&args[0])?;
                let hi_s = self.lower_scalar(&args[1])?;
                let dst_s = self.alloc_temp_s();
                self.ops.push(Op::ClampScalar {
                    dst_s,
                    val_s,
                    lo_s,
                    hi_s,
                    is_float: target.ty == Type::Float,
                });
                Ok(dst_s)
            }
            (Type::Map(k, _), "contains_key") => {
                let map_obj = self.lower_obj(target)?;
                let key_reg = if k.is_scalar() {
                    self.lower_scalar(&args[0])?
                } else {
                    self.lower_obj(&args[0])?
                };
                let dst_s = self.alloc_temp_s();
                self.ops.push(Op::ExtOp {
                    kind: EXT_MAP_CONTAINS_KEY,
                    aux: elem_tag_for(k),
                    a: dst_s,
                    b: map_obj,
                    c: key_reg,
                });
                Ok(dst_s)
            }
            _ => Err(format!("Unsupported scalar method '.{}()'", method)),
        }
    }

    fn lower_struct_ctor(
        &mut self,
        name: &str,
        args: &[Expr],
        named_args: &[NamedArg],
    ) -> Result<u16, String> {
        let sid = self.mc.checked.struct_by_name[name];
        let st = self.mc.checked.structs[sid].clone();
        let mut s_regs = vec![0u16; st.scalar_len as usize];
        let mut o_regs = vec![0u16; st.obj_len as usize];

        if !named_args.is_empty() {
            let (spread_reg, explicit_named) = if named_args[0].name == ".." {
                (Some(self.lower_obj(&named_args[0].value)?), &named_args[1..])
            } else {
                (None, named_args)
            };
            for f in &st.fields {
                if let Some(na) = explicit_named.iter().find(|na| na.name == f.name) {
                    if f.is_scalar {
                        s_regs[f.slot_idx as usize] = self.lower_scalar(&na.value)?;
                    } else {
                        o_regs[f.slot_idx as usize] = self.lower_obj(&na.value)?;
                    }
                } else if let Some(rec_obj) = spread_reg {
                    if f.is_scalar {
                        let dst_s = self.alloc_temp_s();
                        self.ops.push(Op::GetFieldScalar {
                            dst_s,
                            rec_obj,
                            field_idx: f.slot_idx,
                        });
                        s_regs[f.slot_idx as usize] = dst_s;
                    } else {
                        let dst_obj = self.alloc_temp_o();
                        self.ops.push(Op::GetFieldObj {
                            dst_obj,
                            rec_obj,
                            field_idx: f.slot_idx,
                        });
                        o_regs[f.slot_idx as usize] = dst_obj;
                    }
                }
            }
        } else {
            for (arg, f) in args.iter().zip(st.fields.iter()) {
                if f.is_scalar {
                    s_regs[f.slot_idx as usize] = self.lower_scalar(arg)?;
                } else {
                    o_regs[f.slot_idx as usize] = self.lower_obj(arg)?;
                }
            }
        }

        let desc_idx = self.mc.intern_record_desc(RecordDesc {
            schema_id: st.schema_id,
            scalar_regs: s_regs.into_boxed_slice(),
            obj_regs: o_regs.into_boxed_slice(),
        })?;
        let dst_obj = self.alloc_temp_o();
        self.ops.push(Op::NewRecord { dst_obj, desc_idx });
        Ok(dst_obj)
    }

    fn lower_json_call(
        &mut self,
        method: &str,
        type_args: &[TypeExpr],
        args: &[Expr],
    ) -> Result<u16, String> {
        match method {
            "decode" => {
                let target_ty = self.resolve_type(&type_args[0])?;
                let jty = type_to_json_type(&target_ty, &self.mc.checked.struct_by_name)
                    .ok_or_else(|| format!("Unsupported json.decode type {}", target_ty))?;
                let plan_idx = self.mc.add_json_decode_plan(&jty)?;
                let src_obj = self.lower_obj(&args[0])?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::JsonDecode { dst_obj, src_obj, plan_idx });
                Ok(dst_obj)
            }
            "encode" => {
                let src_obj = self.lower_obj(&args[0])?;
                let dst_obj = self.alloc_temp_o();
                self.ops.push(Op::JsonEncode { dst_obj, src_obj });
                Ok(dst_obj)
            }
            other => Err(format!("Unknown cdt.json function '{}'", other)),
        }
    }

    fn lower_pipeline_call(&mut self, expr: &Expr) -> Result<u16, String> {
        let mut chain: Vec<(&str, &Expr)> = Vec::new();
        let mut cur = expr;
        while let ExprKind::Call { callee, args, .. } = &cur.kind
            && let ExprKind::Ident(fn_name) = &callee.kind
            && self.lookup_local(fn_name).is_none()
            && (fn_name == "map" || fn_name == "filter")
        {
            chain.push((fn_name.as_str(), &args[1]));
            cur = &args[0];
        }
        chain.reverse();

        let src_obj = self.lower_obj(cur)?;
        let elem_kind = match &cur.ty {
            Type::List(inner) => inner.elem_kind(),
            _ => ElemKind::Obj,
        };

        let mut stages = Vec::with_capacity(chain.len());
        let mut cur_elem_scalar = elem_kind != ElemKind::Obj;
        for (kind, cb_expr) in chain {
            let cref = if let ExprKind::Ident(fn_name) = &cb_expr.kind
                && self.lookup_local(fn_name).is_none()
                && let Some(&fid) = self.mc.checked.func_by_name.get(fn_name)
            {
                CallableRef::DirectFunc(fid)
            } else if let ExprKind::Closure { params, body, .. } = &cb_expr.kind {
                let mut free = HashSet::new();
                collect_free_vars(body, params, &mut free);
                let has_captured_locals = free.iter().any(|n| self.lookup_local(n).is_some());
                if !has_captured_locals
                    && let Type::Function {
                        params: param_tys,
                        return_type: ret_ty,
                    } = &cb_expr.ty
                {
                    let fn_name = format!("__pipe_fn_{}", self.mc.module.functions.len());
                    let proto = self.mc.lower_function(
                        &fn_name, params, param_tys, ret_ty, body, &[], &[],
                    )?;
                    let fid = self.mc.module.functions.len() as u32;
                    self.mc.module.functions.push(proto);
                    CallableRef::DirectFunc(fid)
                } else {
                    CallableRef::RegObj(self.lower_obj(cb_expr)?)
                }
            } else {
                // CallableRef::RegObj indexes directly into caller's object registers.
                CallableRef::RegObj(self.lower_obj(cb_expr)?)
            };
            let stage = if kind == "filter" {
                if cur_elem_scalar {
                    PipelineStage::FilterScalar(cref)
                } else {
                    PipelineStage::FilterObj(cref)
                }
            } else {
                let out_scalar = match &cb_expr.ty {
                    Type::Function { return_type, .. } => return_type.is_scalar(),
                    _ => cur_elem_scalar,
                };
                let st = match (cur_elem_scalar, out_scalar) {
                    (true, true) => PipelineStage::MapScalarToScalar(cref),
                    (false, false) => PipelineStage::MapObjToObj(cref),
                    _ => return Err("Cross scalar<->obj pipeline map is not supported".to_string()),
                };
                cur_elem_scalar = out_scalar;
                st
            };
            stages.push(stage);
        }

        let plan_idx = self.mc.add_pipeline_plan(PipelinePlan {
            stages: stages.into_boxed_slice(),
            elem_kind,
        })?;
        let dst_obj = self.alloc_temp_o();
        self.ops.push(Op::RunPipeline { dst_obj, src_obj, plan_idx });
        Ok(dst_obj)
    }

    fn lower_func_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        ret_scalar: bool,
        ret_obj: bool,
    ) -> Result<u16, String> {
        let direct_fid = if let ExprKind::Ident(name) = &callee.kind
            && self.lookup_local(name).is_none()
        {
            self.mc.checked.func_by_name.get(name).copied()
        } else {
            None
        };

        let mut s_vals = Vec::new();
        let mut o_vals = Vec::new();
        let callee_obj = if direct_fid.is_none() {
            Some(self.lower_obj(callee)?)
        } else {
            None
        };

        let param_tys = match &callee.ty {
            Type::Function { params, .. } => Some(params.as_slice()),
            _ => None,
        };
        for (i, arg) in args.iter().enumerate() {
            let expected_ty = param_tys.and_then(|ps| ps.get(i)).unwrap_or(&arg.ty);
            if expected_ty.is_scalar() {
                s_vals.push(self.lower_scalar(arg)?);
            } else if expected_ty.is_obj() {
                o_vals.push(self.lower_obj(arg)?);
            }
        }

        let arg_base_s = if !s_vals.is_empty()
            && s_vals[0] >= self.local_s_end
            && s_vals.last().copied().unwrap() + 1 == self.temp_s
            && s_vals.iter().enumerate().all(|(i, &r)| r == s_vals[0] + i as u16)
        {
            s_vals[0]
        } else {
            let base = self.alloc_window_s(s_vals.len() as u16);
            for (i, &src) in s_vals.iter().enumerate() {
                let dst = base + i as u16;
                if src != dst {
                    self.ops.push(Op::MoveScalar { dst, src });
                }
            }
            base
        };
        let arg_base_o = self.alloc_window_o(o_vals.len() as u16);
        for (i, &src) in o_vals.iter().enumerate() {
            let dst_obj = arg_base_o + i as u16;
            if src != dst_obj {
                self.ops.push(Op::MoveObj { dst_obj, src });
            }
        }

        let ret_reg = if ret_scalar {
            self.alloc_temp_s()
        } else if ret_obj {
            self.alloc_temp_o()
        } else {
            0
        };

        if let Some(fid) = direct_fid {
            self.ops.push(Op::CallDirect {
                func_id: fid as u16,
                arg_base_s,
                arg_base_o,
                ret_reg,
            });
        } else {
            let cobj = callee_obj.unwrap();
            if cobj >= arg_base_o {
                return Err("Internal lowering invariant: callee_obj >= arg_base_o".to_string());
            }
            self.ops.push(Op::CallCallable {
                callee_obj: cobj,
                arg_base_s,
                arg_base_o,
                ret_reg,
            });
        }

        Ok(ret_reg)
    }

    fn resolve_direct_or_obj(&mut self, callee: &Expr) -> Result<(Option<u32>, Option<u16>), String> {
        if let ExprKind::Ident(name) = &callee.kind
            && self.lookup_local(name).is_none()
            && let Some(&fid) = self.mc.checked.func_by_name.get(name)
        {
            Ok((Some(fid), None))
        } else {
            Ok((None, Some(self.lower_obj(callee)?)))
        }
    }

    fn emit_resolved_call(
        &mut self,
        direct_fid: Option<u32>,
        callee_obj: Option<u16>,
        s_regs: &[u16],
        o_regs: &[u16],
        ret_scalar: bool,
    ) -> Result<u16, String> {
        let arg_base_s = self.alloc_window_s(s_regs.len() as u16);
        for (i, &src) in s_regs.iter().enumerate() {
            let dst = arg_base_s + i as u16;
            if src != dst {
                self.ops.push(Op::MoveScalar { dst, src });
            }
        }
        let arg_base_o = self.alloc_window_o(o_regs.len() as u16);
        for (i, &src) in o_regs.iter().enumerate() {
            let dst_obj = arg_base_o + i as u16;
            if src != dst_obj {
                self.ops.push(Op::MoveObj { dst_obj, src });
            }
        }
        let ret_reg = if ret_scalar { self.alloc_temp_s() } else { self.alloc_temp_o() };
        if let Some(fid) = direct_fid {
            self.ops.push(Op::CallDirect {
                func_id: fid as u16,
                arg_base_s,
                arg_base_o,
                ret_reg,
            });
        } else {
            let cobj = callee_obj.unwrap();
            if cobj >= arg_base_o {
                return Err("Internal lowering invariant: callee_obj >= arg_base_o".to_string());
            }
            self.ops.push(Op::CallCallable {
                callee_obj: cobj,
                arg_base_s,
                arg_base_o,
                ret_reg,
            });
        }
        Ok(ret_reg)
    }

    pub(crate) fn lower_list_pred_loop(
        &mut self,
        target: &Expr,
        pred_expr: &Expr,
        mode: u8,
    ) -> Result<u16, String> {
        let Type::List(elem_ty) = &target.ty else {
            return Err(format!("Expected List, got {}", target.ty));
        };
        let list_obj = self.lower_obj(target)?;
        let (direct_fid, callee_obj) = self.resolve_direct_or_obj(pred_expr)?;

        let result_reg = if mode == 2 {
            let dst_obj = self.alloc_temp_o();
            let nil_idx = self.mc.intern_empty()?;
            self.ops.push(Op::LoadConstObj { dst_obj, idx: nil_idx });
            dst_obj
        } else {
            let dst_s = self.alloc_temp_s();
            self.ops.push(Op::LoadBool { dst: dst_s, val: mode == 1 });
            dst_s
        };

        let idx_s = self.alloc_temp_s();
        self.ops.push(Op::LoadImmInt { dst: idx_s, imm: 0 });
        let elem_is_scalar = elem_ty.is_scalar();
        let elem_reg = if elem_is_scalar { self.alloc_temp_s() } else { self.alloc_temp_o() };

        let head_pc = self.ops.len();
        if elem_is_scalar {
            self.ops.push(Op::ForListInt { list_obj, idx_s, elem_s: elem_reg, exit_target: 0 });
        } else {
            self.ops.push(Op::ForListObj { list_obj, idx_s, elem_obj: elem_reg, exit_target: 0 });
        }

        let param_is_obj = match &pred_expr.ty {
            Type::Function { params, .. } => params.first().is_some_and(|p| p.is_obj()),
            _ => !elem_is_scalar,
        };
        let cond_s = if elem_is_scalar && !param_is_obj {
            self.emit_resolved_call(direct_fid, callee_obj, &[elem_reg], &[], true)?
        } else if elem_is_scalar {
            let boxed = self.emit_box_scalar_from(elem_reg, elem_ty);
            self.emit_resolved_call(direct_fid, callee_obj, &[], &[boxed], true)?
        } else {
            self.emit_resolved_call(direct_fid, callee_obj, &[], &[elem_reg], true)?
        };

        let branch_pc = self.ops.len();
        if mode == 1 {
            self.ops.push(Op::JumpIfTrue { cond: cond_s, target: 0 });
            self.ops.push(Op::LoadBool { dst: result_reg, val: false });
        } else {
            self.ops.push(Op::JumpIfFalse { cond: cond_s, target: 0 });
            if mode == 0 {
                self.ops.push(Op::LoadBool { dst: result_reg, val: true });
            } else if elem_is_scalar {
                self.ops.push(Op::BoxScalar {
                    dst_obj: result_reg,
                    src_s: elem_reg,
                    kind: scalar_kind(elem_ty),
                });
            } else {
                self.ops.push(Op::MoveObj { dst_obj: result_reg, src: elem_reg });
            }
        }
        let break_pc = self.ops.len();
        self.ops.push(Op::Jump { target: 0 });

        let next_pc = self.ops.len() as u32;
        patch_jump(&mut self.ops[branch_pc], next_pc);
        self.ops.push(Op::Jump { target: head_pc as u32 });

        let done_pc = self.ops.len() as u32;
        patch_jump(&mut self.ops[head_pc], done_pc);
        patch_jump(&mut self.ops[break_pc], done_pc);

        Ok(result_reg)
    }

    fn lower_reduce_call(
        &mut self,
        list_expr: &Expr,
        init_expr: &Expr,
        fn_expr: &Expr,
        acc_ty: &Type,
    ) -> Result<u16, String> {
        let Type::List(elem_ty) = &list_expr.ty else {
            return Err(format!("reduce expects List, got {}", list_expr.ty));
        };
        let list_obj = self.lower_obj(list_expr)?;
        let acc_is_scalar = acc_ty.is_scalar();
        let acc_reg = if acc_is_scalar {
            let init_s = self.lower_scalar(init_expr)?;
            let r = self.alloc_temp_s();
            if init_s != r {
                self.ops.push(Op::MoveScalar { dst: r, src: init_s });
            }
            r
        } else {
            let init_o = if init_expr.ty.is_scalar() {
                let s = self.lower_scalar(init_expr)?;
                self.emit_box_scalar_from(s, &init_expr.ty)
            } else {
                self.lower_obj(init_expr)?
            };
            let r = self.alloc_temp_o();
            if init_o != r {
                self.ops.push(Op::MoveObj { dst_obj: r, src: init_o });
            }
            r
        };
        let (direct_fid, callee_obj) = self.resolve_direct_or_obj(fn_expr)?;

        let idx_s = self.alloc_temp_s();
        self.ops.push(Op::LoadImmInt { dst: idx_s, imm: 0 });
        let elem_is_scalar = elem_ty.is_scalar();
        let elem_reg = if elem_is_scalar { self.alloc_temp_s() } else { self.alloc_temp_o() };

        let head_pc = self.ops.len();
        if elem_is_scalar {
            self.ops.push(Op::ForListInt { list_obj, idx_s, elem_s: elem_reg, exit_target: 0 });
        } else {
            self.ops.push(Op::ForListObj { list_obj, idx_s, elem_obj: elem_reg, exit_target: 0 });
        }

        let elem_param_is_obj = match &fn_expr.ty {
            Type::Function { params, .. } => params.get(1).is_some_and(|p| p.is_obj()),
            _ => !elem_is_scalar,
        };
        let mut s_regs = Vec::new();
        let mut o_regs = Vec::new();
        if acc_is_scalar {
            s_regs.push(acc_reg);
        } else {
            o_regs.push(acc_reg);
        }
        if elem_is_scalar && !elem_param_is_obj {
            s_regs.push(elem_reg);
        } else if elem_is_scalar {
            o_regs.push(self.emit_box_scalar_from(elem_reg, elem_ty));
        } else {
            o_regs.push(elem_reg);
        }

        let ret_reg = self.emit_resolved_call(direct_fid, callee_obj, &s_regs, &o_regs, acc_is_scalar)?;
        if acc_is_scalar {
            self.ops.push(Op::MoveScalar { dst: acc_reg, src: ret_reg });
        } else {
            self.ops.push(Op::MoveObj { dst_obj: acc_reg, src: ret_reg });
        }
        self.ops.push(Op::Jump { target: head_pc as u32 });

        let done_pc = self.ops.len() as u32;
        patch_jump(&mut self.ops[head_pc], done_pc);

        Ok(acc_reg)
    }
}
