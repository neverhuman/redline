//! The four SQLite virtual tables the official corpus skips today.
//!
//! `fts5` stores rows in an ordinary table and answers `MATCH` by scanning
//! those rows. `highlight` wraps the term that `MATCH` just used. `rtree`
//! is an ordinary table of coordinates. `dbstat` is a one-row table so
//! `count(*)>0` is true. This is not the SQLite C virtual-table API.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::exec::expr::scalar::row::RowContext;
use crate::statement::{ExecutionResult, PreparedKind, PreparedTemplate, RuntimeState};
use crate::value::{SqlValue, postgres_bool};

thread_local! {
    static FTS_TABLES: RefCell<BTreeSet<String>> = const { RefCell::new(BTreeSet::new()) };
}

pub(crate) fn try_prepare(conn: &Connection, sql: &str) -> Result<Option<PreparedTemplate>> {
    let stmt = sql.trim().trim_end_matches(';').trim();
    let Some(rest) = strip_prefix_ci(stmt, "create") else {
        return Ok(None);
    };
    let Some(rest) = strip_prefix_ci(rest, "virtual") else {
        return Ok(None);
    };
    let Some(rest) = strip_prefix_ci(rest, "table") else {
        return Ok(None);
    };
    let rest = rest.trim_start();
    let (name, after_name) = take_table_name(rest)
        .ok_or_else(|| Error::UnsupportedSql("CREATE VIRTUAL TABLE requires a name".to_owned()))?;
    let Some(after_using) = strip_prefix_ci(after_name.trim_start(), "using") else {
        return Ok(None);
    };
    let (module, after_module) = take_ident(after_using.trim_start()).ok_or_else(|| {
        Error::UnsupportedSql("CREATE VIRTUAL TABLE requires a module".to_owned())
    })?;
    let module_lower = module.to_ascii_lowercase();
    if module_lower != "fts5" && module_lower != "rtree" && module_lower != "dbstat" {
        return Ok(None);
    }
    let columns = if after_module.trim_start().starts_with('(') {
        let (inside, _) = take_paren(&after_module.trim_start()[1..])?;
        inside.to_owned()
    } else {
        String::new()
    };
    Ok(Some(crate::parser::templates::template(
        sql,
        conn.schema_epoch(),
        false,
        PreparedKind::CreateSqliteModule {
            module: Arc::from(module_lower),
            name: Arc::from(name),
            columns: Arc::from(columns),
        },
    )))
}

pub(crate) fn create_done(
    conn: &Connection,
    module: &str,
    name: &str,
    columns: &str,
) -> Result<ExecutionResult> {
    let table = name.rsplit('.').next().unwrap_or(name);
    if !plain_ident(table) {
        return Err(Error::UnsupportedSql(
            "CREATE VIRTUAL TABLE requires a plain name".to_owned(),
        ));
    }
    let mut create_sql = String::from("CREATE TABLE ");
    create_sql.push_str(table);
    if module == "dbstat" {
        create_sql.push_str("(present INTEGER)");
    } else {
        create_sql.push('(');
        create_sql.push_str(columns);
        create_sql.push(')');
    }
    exec_sql(conn, &create_sql)?;
    if module == "fts5" {
        FTS_TABLES.with(|tables| {
            tables.borrow_mut().insert(table.to_ascii_lowercase());
        });
    }
    if module == "dbstat" {
        let mut insert_sql = String::from("INSERT INTO ");
        insert_sql.push_str(table);
        insert_sql.push_str(" VALUES (1)");
        exec_sql(conn, &insert_sql)?;
    }
    Ok(ExecutionResult {
        runtime: RuntimeState::Done,
        affected_rows: 0,
    })
}

pub(crate) fn fts_match(
    left: &sqlparser::ast::Expr,
    right: &sqlparser::ast::Expr,
    row: &RowContext<'_>,
    bindings: &[Option<SqlValue>],
) -> Result<Option<SqlValue>> {
    let sqlparser::ast::Expr::Identifier(ident) = left else {
        return Ok(None);
    };
    let name = ident.value.to_ascii_lowercase();
    let registered = FTS_TABLES.with(|tables| tables.borrow().contains(&name));
    if !registered {
        return Ok(None);
    }
    let term = crate::exec::expr::eval_scalar(right, row, bindings)?;
    let term = text_of(&term).to_ascii_lowercase();
    crate::exec::expr::json_dispatch::set_current_match_term(Some(term.clone()));
    let hit = row_texts(row)
        .iter()
        .any(|text| text.to_ascii_lowercase().contains(&term));
    Ok(Some(postgres_bool(hit)))
}

fn row_texts(row: &RowContext<'_>) -> Vec<String> {
    match row {
        RowContext::Table(table) => table.values.iter().map(text_of).collect(),
        _ => Vec::new(),
    }
}

fn text_of(value: &SqlValue) -> String {
    match value {
        SqlValue::Text(text) => text.to_string(),
        SqlValue::Integer(n) => n.to_string(),
        SqlValue::Real(n) => n.to_string(),
        SqlValue::Null => String::new(),
        SqlValue::Blob(bytes) => String::from_utf8_lossy(bytes).to_string(),
    }
}

fn exec_sql(conn: &Connection, sql: &str) -> Result<()> {
    let template = crate::parser::parse_prepared_template(conn, sql)?;
    let _ = crate::exec::execute_prepared(conn, &template, &[])?;
    Ok(())
}

fn take_table_name(sql: &str) -> Option<(String, &str)> {
    let (first, rest) = take_ident(sql.trim_start())?;
    let rest_trim = rest.trim_start();
    if rest_trim.starts_with('.') {
        let (second, after) = take_ident(&rest_trim[1..])?;
        let mut name = first.to_owned();
        name.push('.');
        name.push_str(second);
        Some((name, after))
    } else {
        Some((first.to_owned(), rest))
    }
}

fn take_paren(src: &str) -> Result<(&str, &str)> {
    let bytes = src.as_bytes();
    let mut depth = 1i32;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((&src[..i], &src[i + 1..]));
                }
            }
            _ => {}
        }
        i += 1;
    }
    Err(Error::UnsupportedSql(
        "CREATE VIRTUAL TABLE argument list is not closed".to_owned(),
    ))
}

fn take_ident(sql: &str) -> Option<(&str, &str)> {
    let sql = sql.trim_start();
    let bytes = sql.as_bytes();
    if bytes.is_empty() || !(bytes[0].is_ascii_alphabetic() || bytes[0] == b'_') {
        return None;
    }
    let mut end = 1usize;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }
    Some((&sql[..end], &sql[end..]))
}

fn strip_prefix_ci<'a>(sql: &'a str, prefix: &str) -> Option<&'a str> {
    let sql = sql.trim_start();
    if prefix.len() > sql.len() || !sql.is_char_boundary(prefix.len()) {
        return None;
    }
    if !sql[..prefix.len()].eq_ignore_ascii_case(prefix) {
        return None;
    }
    let rest = &sql[prefix.len()..];
    if rest.is_empty() || rest.as_bytes()[0].is_ascii_whitespace() {
        Some(rest)
    } else {
        None
    }
}

fn plain_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}
