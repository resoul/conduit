use std::sync::Arc;

use crate::native::mysql::{MysqlConn, parse_mysql_url};
use crate::native::pg::{PgConn, parse_pg_url};
use crate::native::{CompiledStructField, FieldSlot, JsonType, ScalarKind, StructSchema};
use crate::vm::string::VmString;
use crate::vm::value::{
    ListStorage, MapKey, MapVal, Obj, OrderedMap, SCHEMA_BOXED_BOOL, SCHEMA_BOXED_FLOAT,
    SCHEMA_BOXED_INT, SCHEMA_DB_CONN, SCHEMA_DB_ERROR, SCHEMA_DB_PARAM, StructData,
};

#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Null,
    Int(i64),
    Float(f64),
    Bool(bool),
    Text(String),
}

impl Cell {
    pub fn to_display_string(&self) -> String {
        match self {
            Cell::Null => String::new(),
            Cell::Int(n) => n.to_string(),
            Cell::Float(f) => f.to_string(),
            Cell::Bool(b) => if *b { "true" } else { "false" }.to_string(),
            Cell::Text(s) => s.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub name: String,
    pub is_bool: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueryResult {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Cell>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Postgres,
    Mysql,
}

impl Dialect {
    pub fn name(self) -> &'static str {
        match self {
            Dialect::Postgres => "postgres",
            Dialect::Mysql => "mysql",
        }
    }
}

pub enum SqlConn {
    Postgres(PgConn),
    Mysql(MysqlConn),
}

impl SqlConn {
    pub fn open(url: &str) -> Result<Self, DbErr> {
        if url.starts_with("postgres://") || url.starts_with("postgresql://") {
            let cfg = parse_pg_url(url).map_err(|msg| DbErr::new("invalid_url", msg, ""))?;
            return PgConn::connect(cfg)
                .map(SqlConn::Postgres)
                .map_err(|msg| DbErr::new("connect_error", msg, ""));
        }
        if url.starts_with("mysql://") {
            let cfg = parse_mysql_url(url).map_err(|msg| DbErr::new("invalid_url", msg, ""))?;
            return MysqlConn::connect(cfg)
                .map(SqlConn::Mysql)
                .map_err(|msg| DbErr::new("connect_error", msg, ""));
        }
        Err(DbErr::new(
            "invalid_url",
            format!(
                "Unsupported database URL '{}': expected postgres:// or mysql://",
                redact_url(url)
            ),
            "",
        ))
    }

    pub fn dialect(&self) -> Dialect {
        match self {
            SqlConn::Postgres(_) => Dialect::Postgres,
            SqlConn::Mysql(_) => Dialect::Mysql,
        }
    }

    pub fn ping(&mut self) -> Result<(), DbErr> {
        self.query("SELECT 1", &[]).map(|_| ())
    }

    pub fn execute(&self, sql: &str, params: &[Cell]) -> Result<u64, DbErr> {
        match self {
            SqlConn::Postgres(pg) => pg.execute(sql, params),
            SqlConn::Mysql(my) => my.execute(sql, params),
        }
    }

    pub fn query(&self, sql: &str, params: &[Cell]) -> Result<QueryResult, DbErr> {
        match self {
            SqlConn::Postgres(pg) => pg.query(sql, params),
            SqlConn::Mysql(my) => my.query(sql, params),
        }
    }

    pub fn last_insert_id(&self) -> i64 {
        match self {
            SqlConn::Postgres(_) => 0,
            SqlConn::Mysql(my) => my.last_insert_id() as i64,
        }
    }

    pub fn begin(&self) -> Result<(), DbErr> {
        self.execute("BEGIN", &[]).map(|_| ())
    }

    pub fn commit(&self) -> Result<(), DbErr> {
        self.execute("COMMIT", &[]).map(|_| ())
    }

    pub fn rollback(&self) -> Result<(), DbErr> {
        self.execute("ROLLBACK", &[]).map(|_| ())
    }

    pub fn close(self) {
        match self {
            SqlConn::Postgres(pg) => pg.close(),
            SqlConn::Mysql(my) => my.close(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DbErr {
    pub kind: String,
    pub message: String,
    pub code: String,
}

impl DbErr {
    pub fn new(
        kind: impl Into<String>,
        message: impl Into<String>,
        code: impl Into<String>,
    ) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
            code: code.into(),
        }
    }

    pub fn into_obj(self) -> Obj {
        make_db_error(&self.kind, self.message, &self.code)
    }
}

pub fn redact_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    match authority.rsplit_once('@') {
        Some((userinfo, host)) => match userinfo.split_once(':') {
            Some((user, _)) => format!("{scheme}://{user}:***@{host}{tail}"),
            None => url.to_string(),
        },
        None => url.to_string(),
    }
}

#[inline]
pub fn make_db_error(kind: &str, message: impl Into<String>, code: &str) -> Obj {
    let mut s = StructData::new(SCHEMA_DB_ERROR, 0, 3);
    s.set_obj(0, Obj::str(kind));
    s.set_obj(1, Obj::str(message));
    s.set_obj(2, Obj::str(code));
    Obj::Record(Arc::new(s))
}

#[inline]
pub fn make_db_conn(handle: i64) -> Obj {
    let mut s = StructData::new(SCHEMA_DB_CONN, 1, 0);
    s.set_scalar(0, handle as u64);
    Obj::Record(Arc::new(s))
}

// Param encoding in StructData(SCHEMA_DB_PARAM, 2 scalars, 1 obj):
// scalar 0 = tag (0=Null, 1=Int, 2=Float, 3=Bool, 4=Text)
// scalar 1 = raw scalar bits (for Int/Float/Bool)
// obj 0 = Obj::Str (for Text)
#[inline]
pub fn make_db_param_null() -> Obj {
    let mut s = StructData::new(SCHEMA_DB_PARAM, 2, 1);
    s.set_scalar(0, 0);
    Obj::Record(Arc::new(s))
}

#[inline]
pub fn make_db_param_int(v: i64) -> Obj {
    let mut s = StructData::new(SCHEMA_DB_PARAM, 2, 1);
    s.set_scalar(0, 1);
    s.set_scalar(1, v as u64);
    Obj::Record(Arc::new(s))
}

#[inline]
pub fn make_db_param_float(bits: u64) -> Obj {
    let mut s = StructData::new(SCHEMA_DB_PARAM, 2, 1);
    s.set_scalar(0, 2);
    s.set_scalar(1, bits);
    Obj::Record(Arc::new(s))
}

#[inline]
pub fn make_db_param_bool(b: bool) -> Obj {
    let mut s = StructData::new(SCHEMA_DB_PARAM, 2, 1);
    s.set_scalar(0, 3);
    s.set_scalar(1, u64::from(b));
    Obj::Record(Arc::new(s))
}

#[inline]
pub fn make_db_param_text(text: Obj) -> Obj {
    let mut s = StructData::new(SCHEMA_DB_PARAM, 2, 1);
    s.set_scalar(0, 4);
    s.set_obj(0, text);
    Obj::Record(Arc::new(s))
}

pub fn obj_to_cell(obj: &Obj) -> Result<Cell, String> {
    let Obj::Record(rec) = obj else {
        return Err("Expected db.Param".to_string());
    };
    if rec.schema_id != SCHEMA_DB_PARAM {
        return Err("Expected db.Param".to_string());
    }
    match rec.get_scalar(0) {
        0 => Ok(Cell::Null),
        1 => Ok(Cell::Int(rec.get_scalar(1) as i64)),
        2 => Ok(Cell::Float(f64::from_bits(rec.get_scalar(1)))),
        3 => Ok(Cell::Bool(rec.get_scalar(1) != 0)),
        4 => match rec.get_obj(0) {
            Obj::Str(s) => Ok(Cell::Text(s.as_str().to_string())),
            _ => Err("Expected String in db.text()".to_string()),
        },
        _ => Err("Invalid db.Param tag".to_string()),
    }
}

pub fn query_result_to_maps(res: QueryResult) -> Obj {
    let col_keys: Vec<Arc<VmString>> = res
        .columns
        .into_iter()
        .map(|c| Arc::new(VmString::from(c.name)))
        .collect();
    let mut out = Vec::with_capacity(res.rows.len());
    for row in res.rows {
        let mut map = OrderedMap::with_capacity(col_keys.len());
        for (i, cell) in row.into_iter().enumerate() {
            if let Some(key) = col_keys.get(i) {
                let s = match cell {
                    Cell::Text(s) => s,
                    other => other.to_display_string(),
                };
                map.set(MapKey::Str(Arc::clone(key)), MapVal::Obj(Obj::str(s)));
            }
        }
        out.push(Obj::Map(Arc::new(map)));
    }
    Obj::List(ListStorage::Obj(Arc::new(out)))
}

fn cell_to_int(cell: &Cell) -> Result<i64, String> {
    match cell {
        Cell::Int(n) => Ok(*n),
        Cell::Bool(b) => Ok(i64::from(*b)),
        Cell::Float(f) => Ok(*f as i64),
        Cell::Text(s) => s
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("Cannot convert '{}' to Int", s)),
        Cell::Null => Err("Unexpected NULL for non-optional Int field".to_string()),
    }
}

fn cell_to_float(cell: &Cell) -> Result<f64, String> {
    match cell {
        Cell::Float(f) => Ok(*f),
        Cell::Int(n) => Ok(*n as f64),
        Cell::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Cell::Text(s) => s
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("Cannot convert '{}' to Float", s)),
        Cell::Null => Err("Unexpected NULL for non-optional Float field".to_string()),
    }
}

fn cell_to_bool(cell: &Cell) -> Result<bool, String> {
    match cell {
        Cell::Bool(b) => Ok(*b),
        Cell::Int(n) => Ok(*n != 0),
        Cell::Text(s) => match s.trim().to_ascii_lowercase().as_str() {
            "1" | "t" | "true" | "yes" | "on" => Ok(true),
            "0" | "f" | "false" | "no" | "off" => Ok(false),
            _ => Err(format!("Cannot convert '{}' to Bool", s)),
        },
        Cell::Float(f) => Ok(*f != 0.0),
        Cell::Null => Err("Unexpected NULL for non-optional Bool field".to_string()),
    }
}

fn cell_into_obj_for_type(cell: Cell, ty: &JsonType) -> Result<Obj, String> {
    match ty {
        JsonType::Optional(inner) => {
            if matches!(cell, Cell::Null) {
                Ok(Obj::Empty)
            } else {
                cell_into_obj_for_type(cell, inner)
            }
        }
        JsonType::Int => Ok(Obj::boxed_int(cell_to_int(&cell)?)),
        JsonType::Float => Ok(Obj::boxed_float(cell_to_float(&cell)?.to_bits())),
        JsonType::Bool => Ok(Obj::boxed_bool(cell_to_bool(&cell)?)),
        JsonType::String => match cell {
            Cell::Null => Err("Unexpected NULL for non-optional String field".to_string()),
            Cell::Text(s) => Ok(Obj::str(s)),
            other => Ok(Obj::str(other.to_display_string())),
        },
        other => Err(format!("Unsupported column mapping target type {:?}", other)),
    }
}

pub fn query_result_to_structs(mut res: QueryResult, schema: &StructSchema) -> Result<Obj, String> {
    let mut col_map: Vec<Option<usize>> = Vec::with_capacity(schema.fields.len());
    for f in schema.fields.iter() {
        let pos = res
            .columns
            .iter()
            .position(|c| c.name == f.name || c.name.eq_ignore_ascii_case(&f.name));
        if pos.is_none() && !f.is_optional {
            return Err(format!(
                "Missing column '{}' for struct '{}'",
                f.name, schema.type_name
            ));
        }
        col_map.push(pos);
    }

    let mut out = Vec::with_capacity(res.rows.len());
    for row in &mut res.rows {
        let mut s = StructData::new(schema.schema_id, schema.scalar_len, schema.obj_len);
        for (fi, f) in schema.fields.iter().enumerate() {
            let Some(col_idx) = col_map[fi] else {
                if let FieldSlot::Obj { slot, .. } = f.slot {
                    s.set_obj(slot as usize, Obj::Empty);
                }
                continue;
            };
            let cell = std::mem::replace(&mut row[col_idx], Cell::Null);
            match f.slot {
                FieldSlot::Scalar { kind, slot } => {
                    let raw = match kind {
                        ScalarKind::Int => cell_to_int(&cell)? as u64,
                        ScalarKind::Float => cell_to_float(&cell)?.to_bits(),
                        ScalarKind::Bool => u64::from(cell_to_bool(&cell)?),
                    };
                    s.set_scalar(slot as usize, raw);
                }
                FieldSlot::Obj { slot, is_optional } => {
                    if matches!(cell, Cell::Null) {
                        if is_optional {
                            s.set_obj(slot as usize, Obj::Empty);
                        } else {
                            return Err(format!(
                                "Unexpected NULL for field '{}.{}'",
                                schema.type_name, f.name
                            ));
                        }
                    } else {
                        let val = cell_into_obj_for_type(cell, &f.ty)?;
                        s.set_obj(slot as usize, val);
                    }
                }
            }
        }
        out.push(Obj::Record(Arc::new(s)));
    }
    Ok(Obj::List(ListStorage::Obj(Arc::new(out))))
}

fn sql_col_type(ty: &JsonType, is_auto: bool, is_unique_or_pk: bool, dialect: Dialect) -> (&'static str, bool) {
    let (inner, is_opt) = match ty {
        JsonType::Optional(in_ty) => (in_ty.as_ref(), true),
        other => (other, false),
    };
    let sql_ty = match (inner, dialect, is_auto, is_unique_or_pk) {
        (JsonType::Int, Dialect::Postgres, true, _) => "BIGSERIAL",
        (JsonType::Int, Dialect::Mysql, true, _) => "BIGINT AUTO_INCREMENT",
        (JsonType::Int, _, _, _) => "BIGINT",
        (JsonType::Float, Dialect::Postgres, _, _) => "DOUBLE PRECISION",
        (JsonType::Float, Dialect::Mysql, _, _) => "DOUBLE",
        (JsonType::Bool, _, _, _) => "BOOLEAN",
        (JsonType::String, Dialect::Mysql, _, true) => "VARCHAR(255)",
        (JsonType::String, _, _, _) => "TEXT",
        _ => "TEXT",
    };
    (sql_ty, is_opt)
}

pub fn build_create_table_sqls(
    model: &crate::ir::DbModelMeta,
    schema: &StructSchema,
    dialect: Dialect,
) -> Vec<String> {
    let mut col_defs = Vec::with_capacity(model.columns.len());
    let mut idx_sqls = Vec::new();
    for col in &model.columns {
        let Some(f) = schema.fields.iter().find(|f| f.name == col.field_name) else {
            continue;
        };
        let (sql_ty, is_opt) = sql_col_type(&f.ty, col.is_auto, col.is_unique || col.is_pk, dialect);
        let mut def = format!("{} {}", col.col_name, sql_ty);
        if col.is_pk {
            def.push_str(" PRIMARY KEY");
        }
        if !is_opt && !col.is_pk {
            def.push_str(" NOT NULL");
        }
        if col.is_unique && !col.is_pk {
            def.push_str(" UNIQUE");
        }
        if let Some(d) = &col.default_sql {
            def.push_str(" DEFAULT ");
            def.push_str(d);
        }
        if let Some((ref_tbl, ref_col)) = &col.fk {
            def.push_str(&format!(" REFERENCES {}({})", ref_tbl, ref_col));
        }
        col_defs.push(def);
        if col.is_index {
            idx_sqls.push(format!(
                "CREATE INDEX IF NOT EXISTS idx_{}_{} ON {} ({})",
                model.table_name, col.col_name, model.table_name, col.col_name
            ));
        }
    }
    let mut out = vec![format!(
        "CREATE TABLE IF NOT EXISTS {} ({})",
        model.table_name,
        col_defs.join(", ")
    )];
    out.extend(idx_sqls);
    out
}

pub fn build_drop_table_sql(model: &crate::ir::DbModelMeta) -> String {
    format!("DROP TABLE IF EXISTS {}", model.table_name)
}

fn field_value_to_cell(
    schema: &StructSchema,
    f: &CompiledStructField,
    rec: &StructData,
    is_auto: bool,
    skip_zero_auto: bool,
    op_name: &str,
) -> Result<Option<Cell>, String> {
    match f.slot {
        FieldSlot::Scalar { kind, slot } => {
            let raw = rec.get_scalar(slot as usize);
            match kind {
                ScalarKind::Int => {
                    let iv = raw as i64;
                    if skip_zero_auto && is_auto && iv == 0 {
                        return Ok(None);
                    }
                    Ok(Some(Cell::Int(iv)))
                }
                ScalarKind::Float => Ok(Some(Cell::Float(f64::from_bits(raw)))),
                ScalarKind::Bool => Ok(Some(Cell::Bool(raw != 0))),
            }
        }
        FieldSlot::Obj { slot, .. } => match rec.get_obj(slot as usize) {
            Obj::Empty => Ok(Some(Cell::Null)),
            Obj::Str(s) => Ok(Some(Cell::Text(s.as_str().to_string()))),
            Obj::Record(b) if b.schema_id == SCHEMA_BOXED_INT => {
                Ok(Some(Cell::Int(b.get_scalar(0) as i64)))
            }
            Obj::Record(b) if b.schema_id == SCHEMA_BOXED_FLOAT => {
                Ok(Some(Cell::Float(f64::from_bits(b.get_scalar(0)))))
            }
            Obj::Record(b) if b.schema_id == SCHEMA_BOXED_BOOL => {
                Ok(Some(Cell::Bool(b.get_scalar(0) != 0)))
            }
            _ => Err(format!(
                "Unsupported field '{}.{}' in db.{}()",
                schema.type_name, f.name, op_name
            )),
        },
    }
}

pub fn build_insert_and_cells(
    model: &crate::ir::DbModelMeta,
    schema: &StructSchema,
    rec: &StructData,
) -> Result<(String, Vec<Cell>), String> {
    let mut cols = Vec::new();
    let mut placeholders = Vec::new();
    let mut cells = Vec::new();
    for col in &model.columns {
        let Some(f) = schema.fields.iter().find(|f| f.name == col.field_name) else {
            continue;
        };
        let Some(cell) = field_value_to_cell(schema, f, rec, col.is_auto, true, "insert")? else {
            continue;
        };
        cols.push(col.col_name.clone());
        placeholders.push("?");
        cells.push(cell);
    }
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        model.table_name,
        cols.join(", "),
        placeholders.join(", ")
    );
    Ok((sql, cells))
}

pub fn build_insert_all_and_cells(
    model: &crate::ir::DbModelMeta,
    schema: &StructSchema,
    recs: &[&StructData],
) -> Result<Vec<(String, Vec<Cell>)>, String> {
    if recs.is_empty() {
        return Ok(Vec::new());
    }
    let mut active_cols = Vec::new();
    for col in &model.columns {
        let Some(f) = schema.fields.iter().find(|f| f.name == col.field_name) else {
            continue;
        };
        if col.is_auto {
            let mut any_nonzero = false;
            for rec in recs {
                if field_value_to_cell(schema, f, rec, true, true, "insert_all")?.is_some() {
                    any_nonzero = true;
                    break;
                }
            }
            if !any_nonzero {
                continue;
            }
        }
        active_cols.push((col, f));
    }
    if active_cols.is_empty() {
        return Err(format!(
            "No insertable columns found for model '{}'",
            model.table_name
        ));
    }
    let col_names: Vec<&str> = active_cols.iter().map(|(c, _)| c.col_name.as_str()).collect();
    let row_ph = format!("({})", vec!["?"; active_cols.len()].join(", "));
    let max_rows = (65_535 / active_cols.len()).max(1);
    let mut batches = Vec::with_capacity(recs.len().div_ceil(max_rows));
    for chunk in recs.chunks(max_rows) {
        let mut cells = Vec::with_capacity(chunk.len() * active_cols.len());
        for rec in chunk {
            for (_, f) in &active_cols {
                let cell = field_value_to_cell(schema, f, rec, false, true, "insert_all")?
                    .unwrap_or(Cell::Null);
                cells.push(cell);
            }
        }
        let values_list = vec![row_ph.as_str(); chunk.len()].join(", ");
        let sql = format!(
            "INSERT INTO {} ({}) VALUES {}",
            model.table_name,
            col_names.join(", "),
            values_list
        );
        batches.push((sql, cells));
    }
    Ok(batches)
}

pub fn build_update_and_cells(
    model: &crate::ir::DbModelMeta,
    schema: &StructSchema,
    rec: &StructData,
) -> Result<(String, Vec<Cell>), String> {
    let pk = pk_column_name(model)?;
    let mut assignments = Vec::new();
    let mut cells = Vec::new();
    let mut pk_cell = None;
    for col in &model.columns {
        let Some(f) = schema.fields.iter().find(|f| f.name == col.field_name) else {
            continue;
        };
        let Some(cell) = field_value_to_cell(schema, f, rec, col.is_auto, false, "update")? else {
            continue;
        };
        if col.col_name == pk {
            pk_cell = Some(cell);
        } else {
            assignments.push(format!("{} = ?", col.col_name));
            cells.push(cell);
        }
    }
    let Some(pk_val) = pk_cell else {
        return Err(format!(
            "Model '{}' is missing primary key column '{}'",
            schema.type_name, pk
        ));
    };
    if assignments.is_empty() {
        return Err(format!(
            "Model '{}' has no non-primary-key columns to update",
            schema.type_name
        ));
    }
    cells.push(pk_val);
    let sql = format!(
        "UPDATE {} SET {} WHERE {} = ?",
        model.table_name,
        assignments.join(", "),
        pk
    );
    Ok((sql, cells))
}

pub fn build_count_sql(model: &crate::ir::DbModelMeta) -> String {
    format!("SELECT COUNT(*) FROM {}", model.table_name)
}

pub fn build_find_by_sql(model: &crate::ir::DbModelMeta, column: &str) -> Result<String, String> {
    let target_col = model
        .columns
        .iter()
        .find(|c| c.col_name == column || c.field_name == column)
        .ok_or_else(|| {
            format!(
                "Unknown column '{}' for table '{}'",
                column, model.table_name
            )
        })?;
    let cols: Vec<&str> = model.columns.iter().map(|c| c.col_name.as_str()).collect();
    Ok(format!(
        "SELECT {} FROM {} WHERE {} = ?",
        cols.join(", "),
        model.table_name,
        target_col.col_name
    ))
}

pub fn extract_count_cell(qr: &QueryResult) -> Result<i64, String> {
    let cell = qr
        .rows
        .first()
        .and_then(|r| r.first())
        .ok_or_else(|| "COUNT(*) query returned no rows".to_string())?;
    cell_to_int(cell)
}

pub fn build_select_all_sql(model: &crate::ir::DbModelMeta) -> String {
    let cols: Vec<&str> = model.columns.iter().map(|c| c.col_name.as_str()).collect();
    format!("SELECT {} FROM {}", cols.join(", "), model.table_name)
}

pub fn pk_column_name(model: &crate::ir::DbModelMeta) -> Result<&str, String> {
    if let Some(pk) = model.columns.iter().find(|c| c.is_pk) {
        return Ok(pk.col_name.as_str());
    }
    if let Some(id_col) = model.columns.iter().find(|c| c.col_name == "id") {
        return Ok(id_col.col_name.as_str());
    }
    Err(format!(
        "Model for table '{}' has no @db.pk or 'id' column",
        model.table_name
    ))
}

pub fn build_find_sql(model: &crate::ir::DbModelMeta) -> Result<String, String> {
    let pk = pk_column_name(model)?;
    let cols: Vec<&str> = model.columns.iter().map(|c| c.col_name.as_str()).collect();
    Ok(format!(
        "SELECT {} FROM {} WHERE {} = ? LIMIT 1",
        cols.join(", "),
        model.table_name,
        pk
    ))
}

pub fn build_delete_sql(model: &crate::ir::DbModelMeta) -> Result<String, String> {
    let pk = pk_column_name(model)?;
    Ok(format!("DELETE FROM {} WHERE {} = ?", model.table_name, pk))
}

pub fn any_obj_to_cell(obj: &Obj) -> Result<Cell, String> {
    match obj {
        Obj::Record(rec) if rec.schema_id == SCHEMA_BOXED_INT => {
            Ok(Cell::Int(rec.get_scalar(0) as i64))
        }
        Obj::Record(rec) if rec.schema_id == SCHEMA_BOXED_FLOAT => {
            Ok(Cell::Float(f64::from_bits(rec.get_scalar(0))))
        }
        Obj::Record(rec) if rec.schema_id == SCHEMA_BOXED_BOOL => {
            Ok(Cell::Bool(rec.get_scalar(0) != 0))
        }
        Obj::Str(s) => Ok(Cell::Text(s.as_str().to_string())),
        Obj::Empty => Ok(Cell::Null),
        _ => Err("Expected Int, Float, Bool, or String key for db.find()/find_by()/delete()".to_string()),
    }
}
