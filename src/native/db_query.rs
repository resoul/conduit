use std::sync::Arc;

use crate::ir::DbModelMeta;
use crate::native::db::{Cell, any_obj_to_cell};
use crate::vm::value::{ListStorage, Obj};

#[inline]
pub fn make_db_query(builder: DbQueryBuilder) -> Obj {
    Obj::DbQuery(Arc::new(builder))
}

pub fn cell_list_from_obj(obj: &Obj) -> Result<Vec<Cell>, String> {
    match obj {
        Obj::List(ListStorage::Int(v)) => Ok(v.iter().copied().map(Cell::Int).collect()),
        Obj::List(ListStorage::Float(v)) => Ok(v.iter().copied().map(Cell::Float).collect()),
        Obj::List(ListStorage::Obj(items)) => items.iter().map(any_obj_to_cell).collect(),
        _ => Err("Expected List for where_in() values".to_string()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhereOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Like,
    ILike,
    In,
    IsNull,
    IsNotNull,
}

impl WhereOp {
    pub fn parse_str(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_uppercase().as_str() {
            "=" | "==" => Ok(Self::Eq),
            "!=" | "<>" => Ok(Self::Ne),
            ">" => Ok(Self::Gt),
            ">=" => Ok(Self::Ge),
            "<" => Ok(Self::Lt),
            "<=" => Ok(Self::Le),
            "LIKE" => Ok(Self::Like),
            "ILIKE" => Ok(Self::ILike),
            _ => Err(format!("Unsupported WHERE operator '{}'", s)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderDir {
    Asc,
    Desc,
}

impl OrderDir {
    pub fn parse_str(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "asc" => Ok(Self::Asc),
            "desc" => Ok(Self::Desc),
            _ => Err(format!("Order direction must be 'asc' or 'desc', got '{}'", s)),
        }
    }

    pub fn as_sql(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryMode {
    All,
    First,
    Count,
    Exists,
    Delete,
    Rows,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhereNode {
    Cond {
        col_name: String,
        op: WhereOp,
        param_count: usize,
    },
    And(Vec<WhereNode>),
    Or(Vec<WhereNode>),
}

#[derive(Debug, Clone)]
pub struct DbQueryBuilder {
    pub conn_handle: u64,
    pub model_idx: usize,
    pub select_cols: Vec<String>,
    pub where_root: Option<WhereNode>,
    pub order_by: Vec<(String, OrderDir)>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub params: Vec<Cell>,
    pub error: Option<String>,
}

impl DbQueryBuilder {
    pub fn new(conn_handle: u64, model_idx: usize) -> Self {
        Self {
            conn_handle,
            model_idx,
            select_cols: Vec::new(),
            where_root: None,
            order_by: Vec::new(),
            limit: None,
            offset: None,
            params: Vec::new(),
            error: None,
        }
    }

    pub fn push_where(&mut self, is_or: bool, node: WhereNode) {
        self.where_root = match self.where_root.take() {
            None => Some(node),
            Some(existing) => {
                if is_or {
                    match existing {
                        WhereNode::Or(mut items) => {
                            items.push(node);
                            Some(WhereNode::Or(items))
                        }
                        other => Some(WhereNode::Or(vec![other, node])),
                    }
                } else {
                    match existing {
                        WhereNode::And(mut items) => {
                            items.push(node);
                            Some(WhereNode::And(items))
                        }
                        other => Some(WhereNode::And(vec![other, node])),
                    }
                }
            }
        };
    }

    pub fn build_sql(&self, model: &DbModelMeta, mode: QueryMode, dialect: &str) -> Result<String, String> {
        if let Some(err) = &self.error {
            return Err(err.clone());
        }
        build_query_sql(
            model,
            &self.select_cols,
            self.where_root.as_ref(),
            &self.order_by,
            self.limit,
            self.offset,
            mode,
            dialect,
        )
    }
}

pub fn resolve_model_column<'a>(model: &'a DbModelMeta, col: &str) -> Result<&'a str, String> {
    model
        .columns
        .iter()
        .find(|c| c.col_name == col || c.field_name == col)
        .map(|c| c.col_name.as_str())
        .ok_or_else(|| format!("Unknown column '{}' for table '{}'", col, model.table_name))
}

fn render_where_node(
    node: &WhereNode,
    model: &DbModelMeta,
    dialect: &str,
    out: &mut String,
) -> Result<(), String> {
    match node {
        WhereNode::Cond {
            col_name,
            op,
            param_count,
        } => {
            let resolved = resolve_model_column(model, col_name)?;
            match op {
                WhereOp::Eq => {
                    out.push_str(resolved);
                    out.push_str(" = ?");
                }
                WhereOp::Ne => {
                    out.push_str(resolved);
                    out.push_str(" != ?");
                }
                WhereOp::Gt => {
                    out.push_str(resolved);
                    out.push_str(" > ?");
                }
                WhereOp::Ge => {
                    out.push_str(resolved);
                    out.push_str(" >= ?");
                }
                WhereOp::Lt => {
                    out.push_str(resolved);
                    out.push_str(" < ?");
                }
                WhereOp::Le => {
                    out.push_str(resolved);
                    out.push_str(" <= ?");
                }
                WhereOp::Like => {
                    out.push_str(resolved);
                    out.push_str(" LIKE ?");
                }
                WhereOp::ILike => {
                    if dialect == "mysql" {
                        out.push_str("LOWER(");
                        out.push_str(resolved);
                        out.push_str(") LIKE LOWER(?)");
                    } else {
                        out.push_str(resolved);
                        out.push_str(" ILIKE ?");
                    }
                }
                WhereOp::In => {
                    if *param_count == 0 {
                        out.push_str("1 = 0");
                    } else {
                        out.push_str(resolved);
                        out.push_str(" IN (");
                        for i in 0..*param_count {
                            if i > 0 {
                                out.push_str(", ");
                            }
                            out.push('?');
                        }
                        out.push(')');
                    }
                }
                WhereOp::IsNull => {
                    out.push_str(resolved);
                    out.push_str(" IS NULL");
                }
                WhereOp::IsNotNull => {
                    out.push_str(resolved);
                    out.push_str(" IS NOT NULL");
                }
            }
            Ok(())
        }
        WhereNode::And(items) => {
            if items.is_empty() {
                out.push_str("1 = 1");
                return Ok(());
            }
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(" AND ");
                }
                let needs_parens = matches!(item, WhereNode::Or(_)) && items.len() > 1;
                if needs_parens {
                    out.push('(');
                }
                render_where_node(item, model, dialect, out)?;
                if needs_parens {
                    out.push(')');
                }
            }
            Ok(())
        }
        WhereNode::Or(items) => {
            if items.is_empty() {
                out.push_str("1 = 0");
                return Ok(());
            }
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(" OR ");
                }
                let needs_parens = matches!(item, WhereNode::And(_)) && items.len() > 1;
                if needs_parens {
                    out.push('(');
                }
                render_where_node(item, model, dialect, out)?;
                if needs_parens {
                    out.push(')');
                }
            }
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn build_query_sql(
    model: &DbModelMeta,
    select_cols: &[String],
    where_root: Option<&WhereNode>,
    order_by: &[(String, OrderDir)],
    limit: Option<i64>,
    offset: Option<i64>,
    mode: QueryMode,
    dialect: &str,
) -> Result<String, String> {
    let mut sql = String::new();
    match mode {
        QueryMode::Count => {
            sql.push_str("SELECT COUNT(*) FROM ");
            sql.push_str(&model.table_name);
        }
        QueryMode::Exists => {
            sql.push_str("SELECT 1 FROM ");
            sql.push_str(&model.table_name);
        }
        QueryMode::Delete => {
            sql.push_str("DELETE FROM ");
            sql.push_str(&model.table_name);
        }
        QueryMode::All | QueryMode::First | QueryMode::Rows => {
            sql.push_str("SELECT ");
            if select_cols.is_empty() {
                for (i, c) in model.columns.iter().enumerate() {
                    if i > 0 {
                        sql.push_str(", ");
                    }
                    sql.push_str(&c.col_name);
                }
            } else {
                for (i, col) in select_cols.iter().enumerate() {
                    if i > 0 {
                        sql.push_str(", ");
                    }
                    sql.push_str(resolve_model_column(model, col)?);
                }
            }
            sql.push_str(" FROM ");
            sql.push_str(&model.table_name);
        }
    }

    if let Some(root) = where_root {
        sql.push_str(" WHERE ");
        render_where_node(root, model, dialect, &mut sql)?;
    }

    if matches!(mode, QueryMode::All | QueryMode::First | QueryMode::Rows) && !order_by.is_empty() {
        sql.push_str(" ORDER BY ");
        for (i, (col, dir)) in order_by.iter().enumerate() {
            if i > 0 {
                sql.push_str(", ");
            }
            sql.push_str(resolve_model_column(model, col)?);
            sql.push(' ');
            sql.push_str(dir.as_sql());
        }
    }

    let effective_limit = match mode {
        QueryMode::First => Some(1),
        QueryMode::Exists => Some(1),
        QueryMode::Count | QueryMode::Delete => None,
        QueryMode::All | QueryMode::Rows => limit,
    };

    if let Some(lim) = effective_limit {
        if lim < 0 {
            return Err(format!("LIMIT must be non-negative, got {}", lim));
        }
        sql.push_str(&format!(" LIMIT {}", lim));
    }

    if matches!(mode, QueryMode::All | QueryMode::First | QueryMode::Rows)
        && let Some(off) = offset
    {
        if off < 0 {
            return Err(format!("OFFSET must be non-negative, got {}", off));
        }
        sql.push_str(&format!(" OFFSET {}", off));
    }

    Ok(sql)
}
