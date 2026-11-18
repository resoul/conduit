use std::sync::Arc;

use crate::native::mysql::{MysqlConn, parse_mysql_url};
use crate::native::pg::{PgConn, parse_pg_url};
use crate::native::{FieldSlot, JsonType, ScalarKind, StructSchema};
use crate::vm::string::VmString;
use crate::vm::value::{
    ListStorage, MapKey, MapVal, Obj, OrderedMap, SCHEMA_DB_CONN, SCHEMA_DB_ERROR,
    SCHEMA_DB_PARAM, StructData,
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

pub fn query_result_to_maps(res: &QueryResult) -> Obj {
    let col_keys: Vec<Arc<VmString>> = res
        .columns
        .iter()
        .map(|c| Arc::new(VmString::from(c.name.as_str())))
        .collect();
    let mut out = Vec::with_capacity(res.rows.len());
    for row in &res.rows {
        let mut map = OrderedMap::with_capacity(col_keys.len());
        for (i, cell) in row.iter().enumerate() {
            if let Some(key) = col_keys.get(i) {
                map.set(
                    MapKey::Str(Arc::clone(key)),
                    MapVal::Obj(Obj::str(cell.to_display_string())),
                );
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

fn cell_to_obj_for_type(cell: &Cell, ty: &JsonType) -> Result<Obj, String> {
    match ty {
        JsonType::Optional(inner) => {
            if matches!(cell, Cell::Null) {
                Ok(Obj::Empty)
            } else {
                cell_to_obj_for_type(cell, inner)
            }
        }
        JsonType::Int => Ok(Obj::boxed_int(cell_to_int(cell)?)),
        JsonType::Float => Ok(Obj::boxed_float(cell_to_float(cell)?.to_bits())),
        JsonType::Bool => Ok(Obj::boxed_bool(cell_to_bool(cell)?)),
        JsonType::String => match cell {
            Cell::Null => Err("Unexpected NULL for non-optional String field".to_string()),
            other => Ok(Obj::str(other.to_display_string())),
        },
        other => Err(format!("Unsupported column mapping target type {:?}", other)),
    }
}

pub fn query_result_to_structs(res: &QueryResult, schema: &StructSchema) -> Result<Obj, String> {
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
    for row in &res.rows {
        let mut s = StructData::new(schema.schema_id, schema.scalar_len, schema.obj_len);
        for (fi, f) in schema.fields.iter().enumerate() {
            let Some(col_idx) = col_map[fi] else {
                if let FieldSlot::Obj { slot, .. } = f.slot {
                    s.set_obj(slot as usize, Obj::Empty);
                }
                continue;
            };
            let cell = &row[col_idx];
            match f.slot {
                FieldSlot::Scalar { kind, slot } => {
                    let raw = match kind {
                        ScalarKind::Int => cell_to_int(cell)? as u64,
                        ScalarKind::Float => cell_to_float(cell)?.to_bits(),
                        ScalarKind::Bool => u64::from(cell_to_bool(cell)?),
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
                        let val = cell_to_obj_for_type(cell, &f.ty)?;
                        s.set_obj(slot as usize, val);
                    }
                }
            }
        }
        out.push(Obj::Record(Arc::new(s)));
    }
    Ok(Obj::List(ListStorage::Obj(Arc::new(out))))
}
