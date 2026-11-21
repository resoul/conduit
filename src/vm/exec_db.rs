use std::time::{SystemTime, UNIX_EPOCH};

use crate::ir::Module;
use crate::native::db::{
    Cell, DbErr, Dialect, SqlConn, any_obj_to_cell, build_count_sql, build_create_table_sqls,
    build_delete_sql, build_drop_table_sql, build_find_by_sql, build_find_sql,
    build_insert_all_and_cells, build_insert_and_cells, build_select_all_sql,
    build_update_and_cells, extract_count_cell,
    make_db_conn, make_db_error, make_db_param_bool, make_db_param_float, make_db_param_int,
    make_db_param_null, make_db_param_text, obj_to_cell, query_result_to_maps,
    query_result_to_structs,
};
use crate::native::db_pool::{DbPoolState, make_db_pool};
use crate::native::db_query::{
    DbQueryBuilder, OrderDir, QueryMode, WhereNode, WhereOp, cell_list_from_obj, make_db_query,
};
use crate::vm::exec_cold::err_msg;
use crate::vm::value::{
    ListStorage, MapKey, MapVal, Obj, OrderedMap, SCHEMA_DB_CONN, SCHEMA_DB_ERROR,
    SCHEMA_DB_PARAM, SCHEMA_DB_POOL,
};
use crate::vm::{ExecResult, Permissions, RuntimeError, Vm};

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
pub const DB_UPDATE: u8 = 23;
pub const DB_COUNT: u8 = 24;
pub const DB_FIND_BY: u8 = 25;
pub const DB_QUERY_ONE: u8 = 26;
pub const DB_QUERY_FROM: u8 = 27;
pub const DB_QUERY_SELECT: u8 = 28;
pub const DB_QUERY_WHERE: u8 = 29;
pub const DB_QUERY_WHERE_IN: u8 = 30;
pub const DB_QUERY_WHERE_NULL: u8 = 31;
pub const DB_QUERY_ORDER_BY: u8 = 32;
pub const DB_QUERY_LIMIT_OFFSET: u8 = 33;
pub const DB_QUERY_TO_SQL: u8 = 34;
pub const DB_QUERY_EXEC: u8 = 35;
pub const DB_QUERY_COMBINE_AND: u8 = 36;
pub const DB_QUERY_COMBINE_OR: u8 = 37;
pub const DB_POOL_OPEN: u8 = 38;
pub const DB_POOL_ACQUIRE: u8 = 39;
pub const DB_POOL_WITH: u8 = 40;
pub const DB_POOL_STATS: u8 = 41;
pub const DB_POOL_CLOSE: u8 = 42;
pub const DB_INSERT_ALL: u8 = 43;

fn query_mode_from_u16(v: u16) -> Result<QueryMode, RuntimeError> {
    match v {
        0 => Ok(QueryMode::All),
        1 => Ok(QueryMode::First),
        2 => Ok(QueryMode::Count),
        3 => Ok(QueryMode::Exists),
        4 => Ok(QueryMode::Delete),
        5 => Ok(QueryMode::Rows),
        _ => Err(err_msg("Invalid QueryMode")),
    }
}

fn check_db_url_net_permission(url: &str, perms: &Permissions) -> Result<(), RuntimeError> {
    perms.check_any_net().map_err(|e| err_msg(&e))?;
    if (url.starts_with("postgres://") || url.starts_with("postgresql://"))
        && let Ok(cfg) = crate::native::pg::parse_pg_url(url)
    {
        perms
            .check_net(&cfg.host, cfg.port)
            .map_err(|e| err_msg(&e))?;
    } else if url.starts_with("mysql://")
        && let Ok(cfg) = crate::native::mysql::parse_mysql_url(url)
    {
        perms
            .check_net(&cfg.host, cfg.port)
            .map_err(|e| err_msg(&e))?;
    }
    Ok(())
}

impl Vm {
    fn extract_db_handle(&self, obj: &Obj) -> Result<u64, RuntimeError> {
        let Obj::Record(rec) = obj else {
            return Err(err_msg("Expected db.Connection"));
        };
        if rec.schema_id != SCHEMA_DB_CONN {
            return Err(err_msg("Expected db.Connection"));
        }
        Ok(rec.get_scalar(0))
    }

    fn take_or_clone_db_query(
        &mut self,
        o_base: usize,
        a: u16,
        b: u16,
    ) -> Result<std::sync::Arc<DbQueryBuilder>, RuntimeError> {
        if a == b {
            match std::mem::take(&mut self.objs[o_base + b as usize]) {
                Obj::DbQuery(q) => Ok(q),
                _ => Err(err_msg("Expected db.Query")),
            }
        } else {
            match &self.objs[o_base + b as usize] {
                Obj::DbQuery(q) => Ok(std::sync::Arc::clone(q)),
                _ => Err(err_msg("Expected db.Query")),
            }
        }
    }

    fn extract_db_query_ref<'a>(&self, obj: &'a Obj) -> Result<&'a DbQueryBuilder, RuntimeError> {
        let Obj::DbQuery(q) = obj else {
            return Err(err_msg("Expected db.Query"));
        };
        Ok(q.as_ref())
    }

    fn extract_db_pool_handle(&self, obj: &Obj) -> Result<u64, RuntimeError> {
        let Obj::Record(rec) = obj else {
            return Err(err_msg("Expected db.Pool"));
        };
        if rec.schema_id != SCHEMA_DB_POOL {
            return Err(err_msg("Expected db.Pool"));
        }
        Ok(rec.get_scalar(0))
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
                check_db_url_net_permission(&url, &self.host.permissions)?;
                let res = match SqlConn::open(&url) {
                    Ok(conn) => {
                        let id = self.host.insert_db_conn(conn, None);
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
                let name = match self.host.get_db_conn(id) {
                    Some(c) => c.dialect().name(),
                    None => "closed",
                };
                self.objs[o_base + a as usize] = Obj::str(name);
            }
            DB_LAST_INSERT_ID => {
                let id = self.extract_db_handle(&self.objs[o_base + b as usize])?;
                let last_id = self
                    .host
                    .get_db_conn(id)
                    .map(|c| c.last_insert_id())
                    .unwrap_or(0);
                self.scalars[s_base + a as usize] = last_id as u64;
            }
            DB_CLOSE => {
                let id = self.extract_db_handle(&self.objs[o_base + b as usize])?;
                if let Some((conn, owner)) = self.host.take_db_conn(id) {
                    if let Some(pool) = owner {
                        pool.release(conn);
                    } else {
                        conn.close();
                    }
                }
                self.objs[o_base + a as usize] = Obj::Empty;
            }
            DB_EXEC => {
                let id = self.extract_db_handle(&self.objs[o_base + b as usize])?;
                let (sql, params, _) = self.extract_sql_and_params(&self.objs[o_base + c as usize])?;
                let res = match self.host.get_db_conn(id) {
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
                let res = match self.host.get_db_conn(id) {
                    Some(conn) => match conn.query(&sql, &params) {
                        Ok(qr) => query_result_to_maps(qr),
                        Err(e) => e.into_obj(),
                    },
                    None => make_db_error("closed", "Database connection is closed", ""),
                };
                self.objs[o_base + a as usize] = res;
            }
            DB_QUERY_TYPED | DB_QUERY_ONE => {
                let id = self.extract_db_handle(&self.objs[o_base + b as usize])?;
                let (sql, params, schema_idx) =
                    self.extract_sql_and_params(&self.objs[o_base + c as usize])?;
                let schema = schema_idx
                    .and_then(|idx| module.struct_schemas.get(idx))
                    .ok_or_else(|| err_msg("Missing struct schema in db.query<T>()"))?;
                let res = match self.host.get_db_conn(id) {
                    Some(conn) => match conn.query(&sql, &params) {
                        Ok(qr) => {
                            if aux == DB_QUERY_ONE && qr.rows.is_empty() {
                                Obj::Empty
                            } else {
                                match query_result_to_structs(qr, schema) {
                                    Ok(Obj::List(ListStorage::Obj(items))) if aux == DB_QUERY_ONE => {
                                        items.first().cloned().unwrap_or(Obj::Empty)
                                    }
                                    Ok(list) => list,
                                    Err(msg) => make_db_error("decode_error", msg, ""),
                                }
                            }
                        }
                        Err(e) => e.into_obj(),
                    },
                    None => make_db_error("closed", "Database connection is closed", ""),
                };
                self.objs[o_base + a as usize] = res;
            }
            DB_BEGIN | DB_COMMIT | DB_ROLLBACK => {
                let id = self.extract_db_handle(&self.objs[o_base + b as usize])?;
                let res = match self.host.get_db_conn(id) {
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
                let res = match run_db_migrations_at(self, module, id as usize, s_top, o_top, false) {
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
            DB_CREATE_TABLE | DB_DROP_TABLE | DB_ALL | DB_COUNT => {
                let id = self.extract_db_handle(&self.objs[o_base + b as usize])?;
                let model = &module.db_models[c as usize];
                let schema = module
                    .struct_schemas
                    .iter()
                    .find(|s| s.schema_id == model.schema_id)
                    .ok_or_else(|| err_msg("Missing struct schema for db model op"))?;
                let res = match self.host.get_db_conn(id) {
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
                        DB_DROP_TABLE => {
                            match conn.execute(&build_drop_table_sql(model, conn.dialect()), &[]) {
                                Ok(_) => Obj::Empty,
                                Err(e) => e.into_obj(),
                            }
                        }
                        DB_COUNT => match conn.query(&build_count_sql(model, conn.dialect()), &[]) {
                            Ok(qr) => match extract_count_cell(&qr) {
                                Ok(n) => Obj::boxed_int(n),
                                Err(msg) => make_db_error("decode_error", &msg, ""),
                            },
                            Err(e) => e.into_obj(),
                        },
                        _ => match conn.query(&build_select_all_sql(model, conn.dialect()), &[]) {
                            Ok(qr) => match query_result_to_structs(qr, schema) {
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
            DB_INSERT | DB_INSERT_ALL | DB_UPDATE | DB_FIND | DB_DELETE => {
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
                let res = match self.host.get_db_conn(id) {
                    Some(conn) => match aux {
                        DB_INSERT => {
                            let Obj::Record(row_rec) = &arg_obj else {
                                return Err(err_msg("Expected struct record in db.insert()"));
                            };
                            match build_insert_and_cells(model, schema, row_rec, conn.dialect()) {
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
                        DB_INSERT_ALL => {
                            let Obj::List(ListStorage::Obj(items)) = &arg_obj else {
                                return Err(err_msg("Expected [T] in db.insert_all()"));
                            };
                            let mut recs = Vec::with_capacity(items.len());
                            for item in items.iter() {
                                let Obj::Record(rec) = item else {
                                    return Err(err_msg("Expected struct records in db.insert_all()"));
                                };
                                recs.push(rec.as_ref());
                            }
                            match build_insert_all_and_cells(model, schema, &recs, conn.dialect()) {
                                Ok(batches) => {
                                    let mut total = 0i64;
                                    let mut err_obj = None;
                                    for (sql, cells) in batches {
                                        match conn.execute(&sql, &cells) {
                                            Ok(n) => total += n as i64,
                                            Err(e) => {
                                                err_obj = Some(e.into_obj());
                                                break;
                                            }
                                        }
                                    }
                                    err_obj.unwrap_or_else(|| Obj::boxed_int(total))
                                }
                                Err(msg) => make_db_error("query_error", &msg, ""),
                            }
                        }
                        DB_UPDATE => {
                            let Obj::Record(row_rec) = &arg_obj else {
                                return Err(err_msg("Expected struct record in db.update()"));
                            };
                            match build_update_and_cells(model, schema, row_rec, conn.dialect()) {
                                Ok((sql, cells)) => match conn.execute(&sql, &cells) {
                                    Ok(n) => Obj::boxed_int(n as i64),
                                    Err(e) => e.into_obj(),
                                },
                                Err(msg) => make_db_error("query_error", &msg, ""),
                            }
                        }
                        DB_FIND => {
                            match (build_find_sql(model, conn.dialect()), any_obj_to_cell(&arg_obj)) {
                                (Ok(sql), Ok(cell)) => match conn.query(&sql, &[cell]) {
                                    Ok(qr) => {
                                        if qr.rows.is_empty() {
                                            Obj::Empty
                                        } else {
                                            match query_result_to_structs(qr, schema) {
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
                            }
                        }
                        _ => match (build_delete_sql(model, conn.dialect()), any_obj_to_cell(&arg_obj)) {
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
            DB_FIND_BY => {
                let id = self.extract_db_handle(&self.objs[o_base + b as usize])?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed find_by args"));
                };
                let model_idx = packed.get_scalar(0) as usize;
                let col_name = match packed.get_obj(0) {
                    Obj::Str(s) => s.as_str(),
                    _ => return Err(err_msg("Expected String column name in db.find_by()")),
                };
                let val_obj = packed.get_obj(1);
                let model = &module.db_models[model_idx];
                let schema = module
                    .struct_schemas
                    .iter()
                    .find(|s| s.schema_id == model.schema_id)
                    .ok_or_else(|| err_msg("Missing struct schema for db.find_by()"))?;
                let res = match self.host.get_db_conn(id) {
                    Some(conn) => match (
                        build_find_by_sql(model, col_name, conn.dialect()),
                        any_obj_to_cell(val_obj),
                    ) {
                        (Ok(sql), Ok(cell)) => match conn.query(&sql, &[cell]) {
                            Ok(qr) => match query_result_to_structs(qr, schema) {
                                Ok(list) => list,
                                Err(msg) => make_db_error("decode_error", &msg, ""),
                            },
                            Err(e) => e.into_obj(),
                        },
                        (Err(msg), _) | (_, Err(msg)) => make_db_error("query_error", &msg, ""),
                    },
                    None => make_db_error("closed", "Database connection is closed", ""),
                };
                self.objs[o_base + a as usize] = res;
            }
            DB_QUERY_FROM => {
                let conn_handle = self.extract_db_handle(&self.objs[o_base + b as usize])?;
                let model_idx = c as usize;
                self.objs[o_base + a as usize] =
                    make_db_query(DbQueryBuilder::new(conn_handle, model_idx));
            }
            DB_QUERY_SELECT => {
                let cols = match &self.objs[o_base + c as usize] {
                    Obj::List(ListStorage::Obj(items)) => items
                        .iter()
                        .map(|o| match o {
                            Obj::Str(s) => Ok(s.as_str().to_string()),
                            _ => Err("Query.select() expects [String]".to_string()),
                        })
                        .collect::<Result<Vec<_>, _>>(),
                    _ => Err("Query.select() expects [String]".to_string()),
                };
                let mut q_arc = self.take_or_clone_db_query(o_base, a, b)?;
                let builder = std::sync::Arc::make_mut(&mut q_arc);
                if builder.error.is_none() {
                    match cols {
                        Ok(list) => builder.select_cols = list,
                        Err(e) => builder.error = Some(e),
                    }
                }
                self.objs[o_base + a as usize] = Obj::DbQuery(q_arc);
            }
            DB_QUERY_WHERE => {
                let Obj::Record(packed) = self.objs[o_base + c as usize].clone() else {
                    return Err(err_msg("Expected packed Query.where args"));
                };
                let is_or = packed.get_scalar(0) != 0;
                let col_name = match packed.get_obj(0) {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("Expected String column name in Query.where()")),
                };
                let op_str = match packed.get_obj(1) {
                    Obj::Str(s) => s.as_str(),
                    _ => return Err(err_msg("Expected String operator in Query.where()")),
                };
                let val_obj = packed.get_obj(2);
                let parsed = (WhereOp::parse_str(op_str), any_obj_to_cell(val_obj));
                let mut q_arc = self.take_or_clone_db_query(o_base, a, b)?;
                let builder = std::sync::Arc::make_mut(&mut q_arc);
                if builder.error.is_none() {
                    match parsed {
                        (Ok(WhereOp::Eq), Ok(Cell::Null)) => {
                            builder.push_where(
                                is_or,
                                WhereNode::Cond {
                                    col_name,
                                    op: WhereOp::IsNull,
                                    param_count: 0,
                                },
                            );
                        }
                        (Ok(WhereOp::Ne), Ok(Cell::Null)) => {
                            builder.push_where(
                                is_or,
                                WhereNode::Cond {
                                    col_name,
                                    op: WhereOp::IsNotNull,
                                    param_count: 0,
                                },
                            );
                        }
                        (Ok(op), Ok(cell)) => {
                            builder.params.push(cell);
                            builder.push_where(
                                is_or,
                                WhereNode::Cond {
                                    col_name,
                                    op,
                                    param_count: 1,
                                },
                            );
                        }
                        (Err(e), _) | (_, Err(e)) => builder.error = Some(e),
                    }
                }
                self.objs[o_base + a as usize] = Obj::DbQuery(q_arc);
            }
            DB_QUERY_WHERE_IN => {
                let Obj::Record(packed) = self.objs[o_base + c as usize].clone() else {
                    return Err(err_msg("Expected packed Query.where_in args"));
                };
                let is_or = packed.get_scalar(0) != 0;
                let col_name = match packed.get_obj(0) {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("Expected String column name in Query.where_in()")),
                };
                let cells_res = cell_list_from_obj(packed.get_obj(1));
                let mut q_arc = self.take_or_clone_db_query(o_base, a, b)?;
                let builder = std::sync::Arc::make_mut(&mut q_arc);
                if builder.error.is_none() {
                    match cells_res {
                        Ok(cells) => {
                            let param_count = cells.len();
                            builder.params.extend(cells);
                            builder.push_where(
                                is_or,
                                WhereNode::Cond {
                                    col_name,
                                    op: WhereOp::In,
                                    param_count,
                                },
                            );
                        }
                        Err(e) => builder.error = Some(e),
                    }
                }
                self.objs[o_base + a as usize] = Obj::DbQuery(q_arc);
            }
            DB_QUERY_WHERE_NULL => {
                let Obj::Record(packed) = self.objs[o_base + c as usize].clone() else {
                    return Err(err_msg("Expected packed Query.where_null args"));
                };
                let is_or = packed.get_scalar(0) != 0;
                let is_not_null = packed.get_scalar(1) != 0;
                let col_name = match packed.get_obj(0) {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("Expected String column name in Query.where_null()")),
                };
                let mut q_arc = self.take_or_clone_db_query(o_base, a, b)?;
                let builder = std::sync::Arc::make_mut(&mut q_arc);
                if builder.error.is_none() {
                    builder.push_where(
                        is_or,
                        WhereNode::Cond {
                            col_name,
                            op: if is_not_null {
                                WhereOp::IsNotNull
                            } else {
                                WhereOp::IsNull
                            },
                            param_count: 0,
                        },
                    );
                }
                self.objs[o_base + a as usize] = Obj::DbQuery(q_arc);
            }
            DB_QUERY_ORDER_BY => {
                let Obj::Record(packed) = self.objs[o_base + c as usize].clone() else {
                    return Err(err_msg("Expected packed Query.order_by args"));
                };
                let col_name = match packed.get_obj(0) {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("Expected String column name in Query.order_by()")),
                };
                let dir_str = match packed.get_obj(1) {
                    Obj::Str(s) => s.as_str(),
                    _ => return Err(err_msg("Expected String direction in Query.order_by()")),
                };
                let dir_res = OrderDir::parse_str(dir_str);
                let mut q_arc = self.take_or_clone_db_query(o_base, a, b)?;
                let builder = std::sync::Arc::make_mut(&mut q_arc);
                if builder.error.is_none() {
                    match dir_res {
                        Ok(dir) => builder.order_by.push((col_name, dir)),
                        Err(e) => builder.error = Some(e),
                    }
                }
                self.objs[o_base + a as usize] = Obj::DbQuery(q_arc);
            }
            DB_QUERY_LIMIT_OFFSET => {
                let Obj::Record(packed) = self.objs[o_base + c as usize].clone() else {
                    return Err(err_msg("Expected packed Query limit/offset args"));
                };
                let kind = packed.get_scalar(0);
                let arg1 = packed.get_scalar(1) as i64;
                let arg2 = packed.get_scalar(2) as i64;
                let mut q_arc = self.take_or_clone_db_query(o_base, a, b)?;
                let builder = std::sync::Arc::make_mut(&mut q_arc);
                if builder.error.is_none() {
                    match kind {
                        0 => {
                            if arg1 < 0 {
                                builder.error = Some(format!("Query.limit() must be non-negative, got {}", arg1));
                            } else {
                                builder.limit = Some(arg1);
                            }
                        }
                        1 => {
                            if arg1 < 0 {
                                builder.error = Some(format!("Query.offset() must be non-negative, got {}", arg1));
                            } else {
                                builder.offset = Some(arg1);
                            }
                        }
                        _ => {
                            if arg1 < 1 || arg2 < 1 {
                                builder.error = Some(format!(
                                    "Query.paginate(page, per_page) requires positive integers, got ({}, {})",
                                    arg1, arg2
                                ));
                            } else {
                                builder.limit = Some(arg2);
                                builder.offset = Some((arg1 - 1).saturating_mul(arg2));
                            }
                        }
                    }
                }
                self.objs[o_base + a as usize] = Obj::DbQuery(q_arc);
            }
            DB_QUERY_TO_SQL => {
                let mode = query_mode_from_u16(c)?;
                let builder = self.extract_db_query_ref(&self.objs[o_base + b as usize])?;
                let model = &module.db_models[builder.model_idx];
                let dialect = self
                    .host
                    .get_db_conn(builder.conn_handle)
                    .map(|conn| match conn.dialect() {
                        Dialect::Postgres => "postgres",
                        Dialect::Mysql => "mysql",
                    })
                    .unwrap_or("postgres");
                let sql = builder.build_sql(model, mode, dialect).map_err(|e| err_msg(&e))?;
                self.objs[o_base + a as usize] = Obj::str(sql);
            }
            DB_QUERY_EXEC => {
                let mode = query_mode_from_u16(c)?;
                let builder = self.extract_db_query_ref(&self.objs[o_base + b as usize])?;
                let model = &module.db_models[builder.model_idx];
                let schema = module
                    .struct_schemas
                    .iter()
                    .find(|s| s.schema_id == model.schema_id)
                    .ok_or_else(|| err_msg("Missing struct schema for db.Query"))?;
                let res = match self.host.get_db_conn(builder.conn_handle) {
                    Some(conn) => {
                        let dialect = match conn.dialect() {
                            Dialect::Postgres => "postgres",
                            Dialect::Mysql => "mysql",
                        };
                        match builder.build_sql(model, mode, dialect) {
                            Ok(sql) => match mode {
                                QueryMode::Delete => match conn.execute(&sql, &builder.params) {
                                    Ok(n) => Obj::boxed_int(n as i64),
                                    Err(e) => e.into_obj(),
                                },
                                QueryMode::Count => match conn.query(&sql, &builder.params) {
                                    Ok(qr) => match extract_count_cell(&qr) {
                                        Ok(n) => Obj::boxed_int(n),
                                        Err(msg) => make_db_error("decode_error", &msg, ""),
                                    },
                                    Err(e) => e.into_obj(),
                                },
                                QueryMode::Exists => match conn.query(&sql, &builder.params) {
                                    Ok(qr) => Obj::boxed_bool(!qr.rows.is_empty()),
                                    Err(e) => e.into_obj(),
                                },
                                QueryMode::First => match conn.query(&sql, &builder.params) {
                                    Ok(qr) => {
                                        if qr.rows.is_empty() {
                                            Obj::Empty
                                        } else {
                                            match query_result_to_structs(qr, schema) {
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
                                QueryMode::All => match conn.query(&sql, &builder.params) {
                                    Ok(qr) => match query_result_to_structs(qr, schema) {
                                        Ok(list) => list,
                                        Err(msg) => make_db_error("decode_error", &msg, ""),
                                    },
                                    Err(e) => e.into_obj(),
                                },
                                QueryMode::Rows => match conn.query(&sql, &builder.params) {
                                    Ok(qr) => query_result_to_maps(qr),
                                    Err(e) => e.into_obj(),
                                },
                            },
                            Err(msg) => make_db_error("query_error", &msg, ""),
                        }
                    }
                    None => make_db_error("closed", "Database connection is closed", ""),
                };
                self.objs[o_base + a as usize] = res;
            }
            DB_QUERY_COMBINE_AND | DB_QUERY_COMBINE_OR => {
                let sub = self.extract_db_query_ref(&self.objs[o_base + c as usize])?.clone();
                let mut q_arc = self.take_or_clone_db_query(o_base, a, b)?;
                let builder = std::sync::Arc::make_mut(&mut q_arc);
                if builder.error.is_none() {
                    if let Some(err) = sub.error {
                        builder.error = Some(err);
                    } else if let Some(sub_where) = sub.where_root {
                        let is_or = aux == DB_QUERY_COMBINE_OR;
                        builder.push_where(is_or, sub_where);
                        builder.params.extend(sub.params);
                    }
                }
                self.objs[o_base + a as usize] = Obj::DbQuery(q_arc);
            }
            DB_POOL_OPEN => {
                let Obj::Record(packed) = &self.objs[o_base + b as usize] else {
                    return Err(err_msg("Expected packed db.pool() args"));
                };
                let url = match packed.get_obj(0) {
                    Obj::Str(s) => s.as_str().to_string(),
                    _ => return Err(err_msg("Expected String url in db.pool()")),
                };
                let max_conns = packed.get_scalar(0) as i64;
                let idle_timeout_ms = packed.get_scalar(1) as i64;
                check_db_url_net_permission(&url, &self.host.permissions)?;
                let res = match DbPoolState::open(&url, max_conns, idle_timeout_ms) {
                    Ok(pool) => {
                        let pid = self.host.insert_db_pool(pool);
                        make_db_pool(pid)
                    }
                    Err(e) => e.into_obj(),
                };
                self.objs[o_base + a as usize] = res;
            }
            DB_POOL_ACQUIRE => {
                let pid = self.extract_db_pool_handle(&self.objs[o_base + b as usize])?;
                let pool_opt = self.host.get_db_pool(pid);
                let res = match pool_opt {
                    Some(pool) => match pool.acquire() {
                        Ok(conn) => {
                            let cid = self.host.insert_db_conn(conn, Some(pool));
                            make_db_conn(cid as i64)
                        }
                        Err(e) => e.into_obj(),
                    },
                    None => make_db_error("closed", "Database pool is closed", ""),
                };
                self.objs[o_base + a as usize] = res;
            }
            DB_POOL_WITH => {
                let pid = self.extract_db_pool_handle(&self.objs[o_base + b as usize])?;
                let Obj::Record(packed) = &self.objs[o_base + c as usize] else {
                    return Err(err_msg("Expected packed db.Pool.with() args"));
                };
                let ret_tag = packed.get_scalar(0) as u8;
                let cb = packed.get_obj(0).clone();
                let pool_opt = self.host.get_db_pool(pid);
                let res = match pool_opt {
                    Some(pool) => match pool.acquire() {
                        Ok(conn) => {
                            let cid = self
                                .host
                                .insert_db_conn(conn, Some(std::sync::Arc::clone(&pool)));
                            let conn_obj = make_db_conn(cid as i64);
                            let cb_res = self.invoke_one_obj_arg_cb(
                                module, s_top, o_top, &cb, conn_obj, ret_tag,
                            );
                            if let Some((c, owner)) = self.host.take_db_conn(cid) {
                                if let Some(owner) = owner {
                                    owner.release(c);
                                } else {
                                    pool.release(c);
                                }
                            }
                            cb_res?
                        }
                        Err(e) => e.into_obj(),
                    },
                    None => make_db_error("closed", "Database pool is closed", ""),
                };
                self.objs[o_base + a as usize] = res;
            }
            DB_POOL_STATS => {
                let pid = self.extract_db_pool_handle(&self.objs[o_base + b as usize])?;
                let (idle, in_use, max) = match self.host.get_db_pool(pid) {
                    Some(pool) => pool.stats(),
                    None => (0, 0, 0),
                };
                let mut m = OrderedMap::with_capacity(3);
                m.set(MapKey::Str(std::sync::Arc::new("idle".into())), MapVal::Scalar(idle as u64));
                m.set(MapKey::Str(std::sync::Arc::new("in_use".into())), MapVal::Scalar(in_use as u64));
                m.set(MapKey::Str(std::sync::Arc::new("max".into())), MapVal::Scalar(max as u64));
                self.objs[o_base + a as usize] = Obj::Map(std::sync::Arc::new(m));
            }
            DB_POOL_CLOSE => {
                let pid = self.extract_db_pool_handle(&self.objs[o_base + b as usize])?;
                if let Some(pool) = self.host.take_db_pool(pid) {
                    pool.close();
                }
                self.objs[o_base + a as usize] = Obj::Empty;
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
    vm.host
        .get_db_conn(cid as u64)
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
