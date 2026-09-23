//! `LANGUAGE SQL` functions. The body is a `SELECT` parsed on its own.
//! Call arguments replace matching identifiers, then that `SELECT` runs.

use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::exec::{materialize_prepared_rows, with_session_reentrant};
use crate::parser::{parse_prepared_template, replace_table_ident};
use crate::session::{SessionState, SqlFnDef};
use crate::value::{SqlValue, postgres_result_dialect};

pub(crate) fn create(conn: &Connection, name: &str, def: SqlFnDef) -> Result<()> {
    let name = name.to_ascii_lowercase();
    with_session_reentrant(conn, |session| {
        session.pg_sql_fns.insert(name, def);
        Ok(())
    })
}

pub(crate) fn drop(conn: &Connection, name: &str, if_exists: bool) -> Result<()> {
    let name = name.to_ascii_lowercase();
    with_session_reentrant(conn, |session| {
        let removed_sql = session.pg_sql_fns.remove(&name).is_some();
        let removed_pl = session.pg_pl_fns.remove(&name).is_some();
        if !removed_sql && !removed_pl && !if_exists {
            return Err(Error::UnsupportedSql(format!(
                "function \"{name}\" does not exist"
            )));
        }
        Ok(())
    })
}

pub(crate) fn try_call(
    conn: &Connection,
    name: &str,
    values: &[SqlValue],
) -> Option<Result<SqlValue>> {
    let def =
        with_session_reentrant(conn, |session| Ok(session.pg_sql_fns.get(name).cloned())).ok()?;
    let def = def?;
    Some(eval_call(conn, &def, values))
}

pub(crate) fn snapshot_tx(session: &mut SessionState) {
    session.pg_sql_fns_tx_snapshot = Some(session.pg_sql_fns.clone());
}

pub(crate) fn release_tx(session: &mut SessionState) {
    session.pg_sql_fns_tx_snapshot = None;
}

pub(crate) fn restore_tx(session: &mut SessionState) {
    if let Some(snapshot) = session.pg_sql_fns_tx_snapshot.take() {
        session.pg_sql_fns = snapshot;
    }
}

pub(crate) fn restore_tx_keep(session: &mut SessionState) {
    if let Some(snapshot) = session.pg_sql_fns_tx_snapshot.as_ref() {
        session.pg_sql_fns = snapshot.clone();
    }
}

pub(crate) fn sql_reads_pg_proc(sql: &str) -> bool {
    sql.to_ascii_lowercase().contains("pg_proc")
}

pub(crate) fn rewrite_pg_proc(conn: &Connection, sql: &str) -> Option<String> {
    if !postgres_result_dialect() {
        return None;
    }
    let lower = sql.to_ascii_lowercase();
    if !from_pg_proc(&lower) {
        return None;
    }
    let rows = with_session_reentrant(conn, |session| {
        Ok(session
            .pg_sql_fns
            .iter()
            .map(|(name, def)| (name.clone(), def.security_definer))
            .collect::<Vec<_>>())
    })
    .ok()?;
    let source = proc_source(&rows);
    let out = replace_table_ident(sql, "pg_proc", &source);
    if out == sql { None } else { Some(out) }
}

fn eval_call(conn: &Connection, def: &SqlFnDef, values: &[SqlValue]) -> Result<SqlValue> {
    if values.len() > def.arg_names.len() {
        return Err(Error::UnsupportedSql(
            "too many arguments for SQL function".to_owned(),
        ));
    }
    let mut literals = Vec::with_capacity(def.arg_names.len());
    for (idx, name) in def.arg_names.iter().enumerate() {
        if let Some(value) = values.get(idx) {
            literals.push((name.clone(), sql_literal(value)));
        } else if let Some(default) = def.defaults.get(idx).and_then(|item| item.clone()) {
            literals.push((name.clone(), default));
        } else {
            return Err(Error::UnsupportedSql(
                "SQL function argument has no default".to_owned(),
            ));
        }
    }
    let substituted = substitute_args(&def.body, &literals);
    let template = parse_prepared_template(conn, &substituted)?;
    let rows = materialize_prepared_rows(conn, &template, &[])?;
    Ok(rows
        .into_iter()
        .next()
        .and_then(|row| row.into_iter().next())
        .unwrap_or(SqlValue::Null))
}

fn substitute_args(body: &str, args: &[(String, String)]) -> String {
    let mut out = String::with_capacity(body.len());
    let bytes = body.as_bytes();
    let mut i = 0usize;
    let mut in_str = false;
    while i < bytes.len() {
        if in_str {
            out.push(bytes[i] as char);
            if bytes[i] == b'\'' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    out.push('\'');
                    i += 2;
                    continue;
                }
                in_str = false;
            }
            i += 1;
            continue;
        }
        if bytes[i] == b'\'' {
            in_str = true;
            out.push('\'');
            i += 1;
            continue;
        }
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let ident = &body[start..i];
            if let Some((_, literal)) = args
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(ident))
            {
                out.push_str(literal);
            } else {
                out.push_str(ident);
            }
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn sql_literal(value: &SqlValue) -> String {
    match value {
        SqlValue::Null => "NULL".to_owned(),
        SqlValue::Integer(n) => n.to_string(),
        SqlValue::Real(n) => n.to_string(),
        SqlValue::Text(text) => {
            let mut out = String::from("'");
            out.push_str(&text.replace('\'', "''"));
            out.push('\'');
            out
        }
        SqlValue::Blob(_) => "NULL".to_owned(),
    }
}

fn proc_source(rows: &[(String, bool)]) -> String {
    if rows.is_empty() {
        return "(SELECT NULL AS proname, NULL AS prosecdef WHERE 0) AS pg_proc".to_owned();
    }
    let mut sql = String::from("(VALUES ");
    for (idx, (name, definer)) in rows.iter().enumerate() {
        if idx > 0 {
            sql.push_str(", ");
        }
        let flag = if *definer { "t" } else { "f" };
        sql.push_str("('");
        sql.push_str(&name.replace('\'', "''"));
        sql.push_str("', '");
        sql.push_str(flag);
        sql.push_str("')");
    }
    sql.push_str(") AS pg_proc(proname, prosecdef)");
    sql
}

fn from_pg_proc(lower: &str) -> bool {
    let needle = " from pg_proc";
    let bytes = lower.as_bytes();
    let needle_bytes = needle.as_bytes();
    let mut i = 0usize;
    while i + needle_bytes.len() <= bytes.len() {
        if &bytes[i..i + needle_bytes.len()] == needle_bytes {
            let after = i + needle_bytes.len();
            if after == bytes.len() || !is_ident_byte(bytes[after]) {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
