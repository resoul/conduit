use std::time::{SystemTime, UNIX_EPOCH};

use crate::ir::Module;
use crate::native::db::{
    Cell, DbErr, Dialect, SqlConn, any_obj_to_cell, build_create_table_sqls, build_delete_sql,
    build_drop_table_sql, build_find_sql, build_insert_and_cells, build_select_all_sql,
    make_db_conn, make_db_error, make_db_param_bool, make_db_param_float, make_db_param_int,
    make_db_param_null, make_db_param_text, obj_to_cell, query_result_to_maps,
    query_result_to_structs,
};
use crate::vm::exec_cold::err_msg;
use crate::vm::value::{ListStorage, Obj, SCHEMA_DB_CONN, SCHEMA_DB_ERROR, SCHEMA_DB_PARAM};
use crate::vm::{ExecResult, RuntimeError, Vm};

pub const DB_CONNECT: u8 = 0;
pub const DB_PARAM_NULL: u8 = 1;
pub const DB_PARAM_INT: u8 = 2;
pub const DB_PARAM_FLOAT: u8 = 3;
pub const DB_PARAM_BOOL: u8 = 4;
pub const DB_PARAM_TEXT: u8 = 5;
pub const DB_DIALECT: u8 = 6;
pub const DB_EXEC: u8 = 7;
pub const DB_QUERY_TYPED: u8 = 8;
pub const DB_QUERY_ROWS: u8 = 15;
pub const DB_BEGIN: u8 = 9;
pub const DB_COMMIT: u8 = 10;
pub const DB_ROLLBACK: u8 = 11;
pub const DB_MIGRATE: u8 = 12;
pub const DB_LAST_INSERT_ID: u8 = 13;
pub const DB_CLOSE: u8 = 14;
pub const DB_CREATE_TABLE: u8 = 16;
pub const DB_DROP_TABLE: u8 = 17;
pub const DB_INSERT: u8 = 18;
pub const DB_ALL: u8 = 19;
pub const DB_FIND: u8 = 20;
pub const DB_DELETE: u8 = 21;
pub const DB_DDL: u8 = 22;

impl Vm {
    fn extract_db_handle(&self, obj: &Obj) -> Result<usize, RuntimeError> {
        let Obj::Record(rec) = obj else {
            return Err(err_msg("Expected db.Connection"));
        };
        if rec.schema_id != SCHEMA_DB_CONN {
            return Err(err_msg("Expected db.Connection"));
        }
        Ok(rec.get_scalar(0) as usize)
    }

    fn extract_sql_and_params(
        &self,
        packed: &Obj,
    ) -> Result<(String, Vec<Cell>, Option<usize>), RuntimeError> {
        let Obj::Record(rec) = packed else {
            return Err(err_msg("Expected packed DB query args"));
        };
        if rec.schema_id != SCHEMA_DB_PARAM {
            return Err(err_msg("Expected packed DB query args"));
        }
        let schema_idx = if rec.scalar_len > 0 {
            Some(rec.get_scalar(0) as usize)
        } else {
            None
        };
        let sql = match rec.get_obj(0) {
            Obj::Str(s) => s.as_str().to_string(),
            _ => return Err(err_msg("Expected String SQL query")),
        };
        let mut params = Vec::new();
        match rec.get_obj(1) {
            Obj::Empty => {}
            Obj::List(ListStorage::Obj(items)) => {
                params.reserve(items.len());
                for item in items.iter() {
                    params.push(obj_to_cell(item).map_err(|m| err_msg(&m))?);
                }
            }
            _ => return Err(err_msg("Expected [db.Param] parameters")),
        }
        Ok((sql, params, schema_idx))
    }

    fn invoke_db_migration_fn(
        &mut self,
        module: &Module,
        s_top: usize,
        o_top: usize,
        func_id: u32,
        cid: usize,
    ) -> Result<Option<Obj>, RuntimeError> {
        let child_s = s_top;
        let child_o = o_top;
        let func = &module.functions[func_id as usize];
        let used_objs = (func.num_objs as usize).max(1);
        self.ensure_capacity(
            child_s + func.num_scalars.max(1) as usize,
            child_o + used_objs,
        );
        self.objs[child_o] = make_db_conn(cid as i64);
        let res = self.exec_from(module, func_id, child_s, child_o, usize::MAX);
        match res {
            Ok(ExecResult::Obj(Obj::Record(rec))) if rec.schema_id == SCHEMA_DB_ERROR => {
                Ok(Some(Obj::Record(rec)))
            }
            Ok(_) => Ok(None),
            Err(e) => {
                self.clear_obj_range(child_o, used_objs);
                Err(e)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exec_db_op(
        &mut self,
        module: &Module,
        s_base: usize,
        o_base: usize,
        s_top: usize,
        o_top: usize,
        aux: u8,
        a: u16,
        b: u16,
        c: u16,
    ) -> Result<(), RuntimeError> {
        match aux {
            DB_CONNECT => {
                let url = self.sys_str(o_base, b, "db.connect()")?;
                let res = match SqlConn::open(&url) {
                    Ok(conn) => {
                        let id = self.host.db_conns.len();
                        self.host.db_conns.push(Some(conn));
                        make_db_conn(id as i64)
                    }
                    Err(e) => e.into_obj(),
                };
                self.objs[o_base + a as usize] = res;
            }
            DB_PARAM_NULL => {
                self.objs[o_base + a as usize] = make_db_param_null();
            }
            DB_PARAM_INT => {
                let v = self.scalars[s_base + b as usize] as i64;
                self.objs[o_base + a as usize] = make_db_param_int(v);
            }
            DB_PARAM_FLOAT => {
                let bits = self.scalars[s_base + b as usize];
                self.objs[o_base + a as usize] = make_db_param_float(bits);
            }
            DB_PARAM_BOOL => {
                let bl = self.scalars[s_base + b as usize] != 0;
                self.objs[o_base + a as usize] = make_db_param_bool(bl);
            }
            DB_PARAM_TEXT => {
                let s = match &self.objs[o_base + b as usize] {
                    Obj::Str(_) => self.objs[o_base + b as usize].clone(),
                    _ => return Err(err_msg("Expected String in db.text()")),
                };
                self.objs[o_base + a as usize] = make_db_param_text(s);
            }
            DB_DIALECT => {
                let id = self.extract_db_handle(&self.objs[o_base + b as usize])?;
                let name = match self.host.db_conns.get(id).and_then(|s| s.as_ref()) {
                    Some(c) => c.dialect().name(),
                    None => "closed",
                };
                self.objs[o_base + a as usize] = Obj::str(name);
            }
            DB_LAST_INSERT_ID => {
                let id = self.extract_db_handle(&self.objs[o_base + b as usize])?;
                let last_id = self
                    .host
                    .db_conns
                    .get(id)
                    .and_then(|s| s.as_ref())
                    .map(|c| c.last_insert_id())
                    .unwrap_or(0);
                self.scalars[s_base + a as usize] = last_id as u64;
            }
            DB_CLOSE => {
                let id = self.extract_db_handle(&self.objs[o_base + b as usize])?;
                if let Some(slot) = self.host.db_conns.get_mut(id)
                    && let Some(conn) = slot.take()
                {
                    conn.close();
                }
                self.objs[o_base + a as usize] = Obj::Empty;
            }
            DB_EXEC => {
                let id = self.extract_db_handle(&self.objs[o_base + b as usize])?;
                let (sql, params, _) = self.extract_sql_and_params(&self.objs[o_base + c as usize])?;
                let res = match self.host.db_conns.get(id).and_then(|s| s.as_ref()) {
                    Some(conn) => match conn.execute(&sql, &params) {
                        Ok(n) => Obj::boxed_int(n as i64),
                        Err(e) => e.into_obj(),
                    },
                    None => make_db_error("closed", "Database connection is closed", ""),
                };
                self.objs[o_base + a as usize] = res;
            }
            DB_QUERY_ROWS => {
                let id = self.extract_db_handle(&self.objs[o_base + b as usize])?;
                let (sql, params, _) = self.extract_sql_and_params(&self.objs[o_base + c as usize])?;
                let res = match self.host.db_conns.get(id).and_then(|s| s.as_ref()) {
                    Some(conn) => match conn.query(&sql, &params) {
                        Ok(qr) => query_result_to_maps(&qr),
                        Err(e) => e.into_obj(),
                    },
                    None => make_db_error("closed", "Database connection is closed", ""),
                };
                self.objs[o_base + a as usize] = res;
            }
            DB_QUERY_TYPED => {
                let id = self.extract_db_handle(&self.objs[o_base + b as usize])?;
                let (sql, params, schema_idx) =
                    self.extract_sql_and_params(&self.objs[o_base + c as usize])?;
                let schema = schema_idx
                    .and_then(|idx| module.struct_schemas.get(idx))
                    .ok_or_else(|| err_msg("Missing struct schema in db.query<T>()"))?;
                let res = match self.host.db_conns.get(id).and_then(|s| s.as_ref()) {
                    Some(conn) => match conn.query(&sql, &params) {
                        Ok(qr) => match query_result_to_structs(&qr, schema) {
                            Ok(list) => list,
                            Err(msg) => make_db_error("decode_error", msg, ""),
                        },
                        Err(e) => e.into_obj(),
                    },
                    None => make_db_error("closed", "Database connection is closed", ""),
                };
                self.objs[o_base + a as usize] = res;
            }
            DB_BEGIN | DB_COMMIT | DB_ROLLBACK => {
                let id = self.extract_db_handle(&self.objs[o_base + b as usize])?;
                let res = match self.host.db_conns.get(id).and_then(|s| s.as_ref()) {
                    Some(conn) => {
                        let r = match aux {
                            DB_BEGIN => conn.begin(),
                            DB_COMMIT => conn.commit(),
                            _ => conn.rollback(),
                        };
                        match r {
                            Ok(()) => Obj::Empty,
                            Err(e) => e.into_obj(),
                        }
                    }
                    None => make_db_error("closed", "Database connection is closed", ""),
                };
                self.objs[o_base + a as usize] = res;
            }
            DB_MIGRATE => {
                let id = self.extract_db_handle(&self.objs[o_base + b as usize])?;
                let res = match run_db_migrations_at(self, module, id, s_top, o_top, false) {
                    Ok(applied) => Obj::boxed_int(applied.len() as i64),
                    Err(EitherMigErr::Db(e)) => e.into_obj(),
                    Err(EitherMigErr::Vm(e)) => return Err(e),
                };
                self.objs[o_base + a as usize] = res;
            }
            DB_DDL => {
                let Obj::Str(d) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected String dialect in db.ddl()"));
                };
                let dialect = match d.as_str() {
                    "mysql" => Dialect::Mysql,
                    _ => Dialect::Postgres,
                };
                let model = &module.db_models[c as usize];
                let schema = module
                    .struct_schemas
                    .iter()
                    .find(|s| s.schema_id == model.schema_id)
                    .ok_or_else(|| err_msg("Missing struct schema for db.ddl()"))?;
                let sql = build_create_table_sqls(model, schema, dialect).join(";\n");
                self.objs[o_base + a as usize] = Obj::str(sql);
            }
            DB_CREATE_TABLE | DB_DROP_TABLE | DB_ALL => {
                let id = self.extract_db_handle(&self.objs[o_base + b as usize])?;
                let model = &module.db_models[c as usize];
                let schema = module
                    .struct_schemas
                    .iter()
                    .find(|s| s.schema_id == model.schema_id)
                    .ok_or_else(|| err_msg("Missing struct schema for db model op"))?;
                let res = match self.host.db_conns.get(id).and_then(|s| s.as_ref()) {
                    Some(conn) => match aux {
                        DB_CREATE_TABLE => {
                            let sqls = build_create_table_sqls(model, schema, conn.dialect());
                            let mut out = Obj::Empty;
                            for sql in sqls {
                                if let Err(e) = conn.execute(&sql, &[]) {
                                    out = e.into_obj();
                                    break;
                                }
                            }
                            out
                        }
                        DB_DROP_TABLE => match conn.execute(&build_drop_table_sql(model), &[]) {
                            Ok(_) => Obj::Empty,
                            Err(e) => e.into_obj(),
                        },
                        _ => match conn.query(&build_select_all_sql(model), &[]) {
                            Ok(qr) => match query_result_to_structs(&qr, schema) {
                                Ok(list) => list,
                                Err(msg) => make_db_error("decode_error", &msg, ""),
                            },
                            Err(e) => e.into_obj(),
                        },
                    },
                    None => make_db_error("closed", "Database connection is closed", ""),
                };
                self.objs[o_base + a as usize] = res;
            }
            DB_INSERT | DB_FIND | DB_DELETE => {
                let id = self.extract_db_handle(&self.objs[o_base + b as usize])?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed model arg"));
                };
                let model_idx = packed.get_scalar(0) as usize;
                let arg_obj = packed.get_obj(0).clone();
                let model = &module.db_models[model_idx];
                let schema = module
                    .struct_schemas
                    .iter()
                    .find(|s| s.schema_id == model.schema_id)
                    .ok_or_else(|| err_msg("Missing struct schema for db model op"))?;
                let res = match self.host.db_conns.get(id).and_then(|s| s.as_ref()) {
                    Some(conn) => match aux {
                        DB_INSERT => {
                            let Obj::Record(row_rec) = &arg_obj else {
                                return Err(err_msg("Expected struct record in db.insert()"));
                            };
                            match build_insert_and_cells(model, schema, row_rec) {
                                Ok((sql, cells)) => match conn.execute(&sql, &cells) {
                                    Ok(n) => {
                                        let lid = conn.last_insert_id();
                                        Obj::boxed_int(if lid > 0 { lid } else { n as i64 })
                                    }
                                    Err(e) => e.into_obj(),
                                },
                                Err(msg) => make_db_error("query_error", &msg, ""),
                            }
                        }
                        DB_FIND => match (build_find_sql(model), any_obj_to_cell(&arg_obj)) {
                            (Ok(sql), Ok(cell)) => match conn.query(&sql, &[cell]) {
                                Ok(qr) => {
                                    if qr.rows.is_empty() {
                                        Obj::Empty
                                    } else {
                                        match query_result_to_structs(&qr, schema) {
                                            Ok(Obj::List(ListStorage::Obj(items))) => {
                                                items.first().cloned().unwrap_or(Obj::Empty)
                                            }
                                            Ok(_) => Obj::Empty,
                                            Err(msg) => make_db_error("decode_error", &msg, ""),
                                        }
                                    }
                                }
                                Err(e) => e.into_obj(),
                            },
                            (Err(msg), _) | (_, Err(msg)) => make_db_error("query_error", &msg, ""),
                        },
                        _ => match (build_delete_sql(model), any_obj_to_cell(&arg_obj)) {
                            (Ok(sql), Ok(cell)) => match conn.execute(&sql, &[cell]) {
                                Ok(n) => Obj::boxed_int(n as i64),
                                Err(e) => e.into_obj(),
                            },
                            (Err(msg), _) | (_, Err(msg)) => make_db_error("query_error", &msg, ""),
                        },
                    },
                    None => make_db_error("closed", "Database connection is closed", ""),
                };
                self.objs[o_base + a as usize] = res;
            }
            _ => return Err(err_msg("Unknown db sub-op")),
        }
        Ok(())
    }
}

pub enum EitherMigErr {
    Db(DbErr),
    Vm(RuntimeError),
}

impl From<DbErr> for EitherMigErr {
    fn from(e: DbErr) -> Self {
        Self::Db(e)
    }
}

impl From<RuntimeError> for EitherMigErr {
    fn from(e: RuntimeError) -> Self {
        Self::Vm(e)
    }
}

fn create_migrations_table_sql(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Postgres => {
            "CREATE TABLE IF NOT EXISTS schema_migrations (\
            id BIGSERIAL PRIMARY KEY, \
            version BIGINT NOT NULL UNIQUE, \
            name TEXT NOT NULL, \
            applied_at BIGINT NOT NULL)"
        }
        Dialect::Mysql => {
            "CREATE TABLE IF NOT EXISTS schema_migrations (\
            id BIGINT AUTO_INCREMENT PRIMARY KEY, \
            version BIGINT NOT NULL UNIQUE, \
            name VARCHAR(255) NOT NULL, \
            applied_at BIGINT NOT NULL)"
        }
    }
}

fn history_exists_sql(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Postgres => {
            "SELECT 1 FROM information_schema.tables WHERE table_schema = current_schema() AND table_name = 'schema_migrations'"
        }
        Dialect::Mysql => {
            "SELECT 1 FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'schema_migrations'"
        }
    }
}

fn load_applied_versions(
    conn: &SqlConn,
    create_if_missing: bool,
) -> Result<Vec<(i64, String, i64)>, DbErr> {
    let dialect = conn.dialect();
    if create_if_missing {
        conn.execute(create_migrations_table_sql(dialect), &[])?;
    } else {
        let chk = conn.query(history_exists_sql(dialect), &[])?;
        if chk.rows.is_empty() {
            return Ok(Vec::new());
        }
    }
    let res = conn.query(
        "SELECT version, name, applied_at FROM schema_migrations ORDER BY version ASC",
        &[],
    )?;
    let mut out = Vec::with_capacity(res.rows.len());
    for row in &res.rows {
        let ver = match &row[0] {
            Cell::Int(n) => *n,
            Cell::Text(s) => s.parse().unwrap_or(0),
            _ => 0,
        };
        let name = row[1].to_display_string();
        let ts = match &row[2] {
            Cell::Int(n) => *n,
            Cell::Text(s) => s.parse().unwrap_or(0),
            _ => 0,
        };
        out.push((ver, name, ts));
    }
    Ok(out)
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn db_conn_ref(vm: &Vm, cid: usize) -> Result<&SqlConn, DbErr> {
    vm.host.db_conns
        .get(cid)
        .and_then(|s| s.as_ref())
        .ok_or_else(|| DbErr::new("closed", "Database connection is closed", ""))
}

pub fn run_db_migrations_at(
    vm: &mut Vm,
    module: &Module,
    cid: usize,
    s_top: usize,
    o_top: usize,
    dry_run: bool,
) -> Result<Vec<(i64, String)>, EitherMigErr> {
    let applied = {
        let conn = db_conn_ref(vm, cid)?;
        load_applied_versions(conn, !dry_run)?
    };
    let mut pending = Vec::new();
    for mig in &module.migrations {
        if !applied.iter().any(|(v, _, _)| *v == mig.version) {
            pending.push((mig.version, mig.name.clone(), mig.up_func_id));
        }
    }
    let mut did = Vec::new();
    for (ver, name, up_fid) in pending {
        if dry_run {
            did.push((ver, name));
            continue;
        }
        db_conn_ref(vm, cid)?.begin()?;
        match vm.invoke_db_migration_fn(module, s_top, o_top, up_fid, cid) {
            Ok(None) => {
                let conn = db_conn_ref(vm, cid)?;
                if let Err(e) = conn.execute(
                    "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?, ?, ?)",
                    &[Cell::Int(ver), Cell::Text(name.clone()), Cell::Int(now_millis())],
                ) {
                    let _ = conn.rollback();
                    return Err(EitherMigErr::Db(e));
                }
                conn.commit()?;
                did.push((ver, name));
            }
            Ok(Some(err_obj)) => {
                let _ = db_conn_ref(vm, cid)?.rollback();
                let msg = match &err_obj {
                    Obj::Record(r) => match r.get_obj(1) {
                        Obj::Str(s) => s.as_str().to_string(),
                        _ => "Migration failed".to_string(),
                    },
                    _ => "Migration failed".to_string(),
                };
                return Err(EitherMigErr::Db(DbErr::new("migration_error", msg, "")));
            }
            Err(vm_err) => {
                let _ = db_conn_ref(vm, cid)?.rollback();
                return Err(EitherMigErr::Vm(vm_err));
            }
        }
    }
    Ok(did)
}

pub fn rollback_db_migrations_at(
    vm: &mut Vm,
    module: &Module,
    cid: usize,
    steps: usize,
    dry_run: bool,
) -> Result<Vec<(i64, String)>, EitherMigErr> {
    let applied = {
        let conn = db_conn_ref(vm, cid)?;
        load_applied_versions(conn, false)?
    };
    let mut targets = Vec::new();
    for (ver, name, _) in applied.iter().rev().take(steps) {
        let mig = module
            .migrations
            .iter()
            .find(|m| m.version == *ver)
            .ok_or_else(|| {
                DbErr::new(
                    "migration_error",
                    format!(
                        "Migration {} '{}' is applied in database but missing from source",
                        ver, name
                    ),
                    "",
                )
            })?;
        targets.push((mig.version, mig.name.clone(), mig.down_func_id));
    }

    let mut did = Vec::new();
    for (ver, name, down_fid) in targets {
        if dry_run {
            did.push((ver, name));
            continue;
        }
        db_conn_ref(vm, cid)?.begin()?;
        match vm.invoke_db_migration_fn(module, 0, 0, down_fid, cid) {
            Ok(None) => {
                let conn = db_conn_ref(vm, cid)?;
                if let Err(e) =
                    conn.execute("DELETE FROM schema_migrations WHERE version = ?", &[Cell::Int(ver)])
                {
                    let _ = conn.rollback();
                    return Err(EitherMigErr::Db(e));
                }
                conn.commit()?;
                did.push((ver, name));
            }
            Ok(Some(err_obj)) => {
                let _ = db_conn_ref(vm, cid)?.rollback();
                let msg = match &err_obj {
                    Obj::Record(r) => match r.get_obj(1) {
                        Obj::Str(s) => s.as_str().to_string(),
                        _ => "Migration rollback failed".to_string(),
                    },
                    _ => "Migration rollback failed".to_string(),
                };
                return Err(EitherMigErr::Db(DbErr::new("migration_error", msg, "")));
            }
            Err(vm_err) => {
                let _ = db_conn_ref(vm, cid)?.rollback();
                return Err(EitherMigErr::Vm(vm_err));
            }
        }
    }
    Ok(did)
}

pub fn db_migration_status_at(
    vm: &Vm,
    module: &Module,
    cid: usize,
) -> Result<Vec<(i64, String, Option<i64>)>, DbErr> {
    let conn = db_conn_ref(vm, cid)?;
    let applied = load_applied_versions(conn, false)?;
    let mut rows = Vec::new();
    for mig in &module.migrations {
        let ts = applied
            .iter()
            .find(|(v, _, _)| *v == mig.version)
            .map(|(_, _, t)| *t);
        rows.push((mig.version, mig.name.clone(), ts));
    }
    for (ver, name, ts) in &applied {
        if !module.migrations.iter().any(|m| m.version == *ver) {
            rows.push((*ver, format!("{} (missing in source)", name), Some(*ts)));
        }
    }
    Ok(rows)
}
