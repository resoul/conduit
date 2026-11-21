use crate::ir::Module;
use crate::native::jwt::{
    JWT_DECODE_UNVERIFIED, JWT_SIGN, JWT_VERIFY, jwt_decode_unverified, jwt_sign, jwt_verify,
};
use crate::vm::exec_cold::err_msg;
use crate::vm::value::Obj;
use crate::vm::{RuntimeError, Vm};

impl Vm {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_jwt_op(
        &mut self,
        module: &Module,
        _s_base: usize,
        o_base: usize,
        aux: u8,
        a: u16,
        b: u16,
        c: u16,
    ) -> Result<(), RuntimeError> {
        match aux {
            JWT_SIGN => {
                let Obj::Record(packed) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected packed arguments for jwt.sign()"));
                };
                let Obj::Str(alg) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String algorithm in jwt.sign()"));
                };
                let claims_obj = packed.get_obj(1);
                let Obj::Str(secret) = packed.get_obj(2) else {
                    return Err(err_msg("Expected String secret in jwt.sign()"));
                };
                let has_ttl = packed.get_scalar(1) != 0;
                let ttl_secs = if has_ttl {
                    Some(packed.get_scalar(0) as i64)
                } else {
                    None
                };
                self.objs[o_base + a as usize] =
                    jwt_sign(alg.as_str(), claims_obj, secret.as_str(), ttl_secs, &module.struct_schemas);
            }
            JWT_VERIFY => {
                let Obj::Record(packed) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected packed arguments for jwt.verify()"));
                };
                let Obj::Str(alg) = packed.get_obj(0) else {
                    return Err(err_msg("Expected String algorithm in jwt.verify()"));
                };
                let Obj::Str(token) = packed.get_obj(1) else {
                    return Err(err_msg("Expected String token in jwt.verify()"));
                };
                let Obj::Str(secret) = packed.get_obj(2) else {
                    return Err(err_msg("Expected String secret in jwt.verify()"));
                };
                let plan_idx = packed.get_scalar(0) as usize;
                let leeway_secs = packed.get_scalar(1) as i64;
                let require_exp = packed.scalar_len >= 3 && packed.get_scalar(2) != 0;
                let expected_iss = if packed.obj_len >= 4 {
                    let Obj::Str(s) = packed.get_obj(3) else {
                        return Err(err_msg("Expected String expected_iss in jwt.verify()"));
                    };
                    if s.is_empty() { None } else { Some(s.as_str()) }
                } else {
                    None
                };
                let expected_aud = if packed.obj_len >= 5 {
                    let Obj::Str(s) = packed.get_obj(4) else {
                        return Err(err_msg("Expected String expected_aud in jwt.verify()"));
                    };
                    if s.is_empty() { None } else { Some(s.as_str()) }
                } else {
                    None
                };
                let Some(plan) = module.json_plans.get(plan_idx) else {
                    return Err(err_msg("JSON decode plan out of bounds in jwt.verify()"));
                };
                self.objs[o_base + a as usize] = jwt_verify(
                    alg.as_str(),
                    token.as_str(),
                    secret.as_str(),
                    leeway_secs,
                    require_exp,
                    expected_iss,
                    expected_aud,
                    plan,
                );
            }
            JWT_DECODE_UNVERIFIED => {
                let token = self.sys_str(o_base, b, "jwt.decode_unverified()")?;
                let Some(plan) = module.json_plans.get(c as usize) else {
                    return Err(err_msg("JSON decode plan out of bounds in jwt.decode_unverified()"));
                };
                self.objs[o_base + a as usize] = jwt_decode_unverified(token.as_str(), plan);
            }
            _ => return Err(err_msg("Unknown EXT_JWT_OP sub-op")),
        }
        Ok(())
    }
}
