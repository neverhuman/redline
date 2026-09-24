//! Publications, `LOCK TABLE`, and the row-lock clauses the corpus uses.
//!
//! `FOR KEY SHARE` and `FOR NO KEY UPDATE` are spellings sqlparser does not
//! accept. They become `FOR SHARE` and `FOR UPDATE`, which this engine
//! already parses and ignores. The rows are the same either way.
//! `pg_export_snapshot` lives with the other session functions.

use std::sync::Arc;

use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::exec::with_session_reentrant;
use crate::session::{BeginMode, SessionState};
use crate::statement::{ExecutionResult, PreparedKind, PreparedTemplate, RuntimeState};

pub(crate) fn try_prepare(conn: &Connection, sql: &str) -> Result<Option<PreparedTemplate>> {
    let stmt = sql.trim().trim_end_matches(';').trim();
    let lower = stmt.to_ascii_lowercase();
    if lower.starts_with("begin ") && lower.contains("isolation level") {
        return Ok(Some(template(
            conn,
            sql,
            PreparedKind::Begin(BeginMode::Deferred),
        )));
    }
    if lower.starts_with("lock table ") {
        return Ok(Some(template(conn, sql, PreparedKind::PgLockTable)));
    }
    if let Some(rest) = strip_prefix_ci(stmt, "create publication") {
        let (name, _) = take_ident(rest.trim_start()).ok_or_else(|| {
            Error::UnsupportedSql("CREATE PUBLICATION requires a name".to_owned())
        })?;
        if !plain_ident(name) {
            return Err(Error::UnsupportedSql(
                "CREATE PUBLICATION requires a name".to_owned(),
            ));
        }
        return Ok(Some(template(
            conn,
            sql,
            PreparedKind::CreatePgPublication {
                name: Arc::from(name.to_ascii_lowercase()),
            },
        )));
    }
    if let Some(rest) = strip_prefix_ci(stmt, "drop publication") {
        let if_exists = strip_prefix_ci(rest.trim_start(), "if exists").is_some();
        let rest = if if_exists {
            strip_prefix_ci(rest.trim_start(), "if exists").unwrap_or(rest)
        } else {
            rest
        };
        let (name, _) = take_ident(rest.trim_start())
            .ok_or_else(|| Error::UnsupportedSql("DROP PUBLICATION requires a name".to_owned()))?;
        return Ok(Some(template(
            conn,
            sql,
            PreparedKind::DropPgPublication {
                name: Arc::from(name.to_ascii_lowercase()),
                if_exists,
            },
        )));
    }
    Ok(None)
}

pub(crate) fn rewrite(conn: &Connection, sql: &str) -> Option<String> {
    let mut out = sql.to_owned();
    let lower = out.to_ascii_lowercase();
    if lower.contains(" for key share") {
        out = replace_ci(&out, " for key share", " for share");
    }
    if out.to_ascii_lowercase().contains(" for no key update") {
        out = replace_ci(&out, " for no key update", " for update");
    }
    if reads_from(&out, "pg_publication") {
        let names =
            with_session_reentrant(conn, |session| Ok(session.pg_publications.clone())).ok()?;
        if !names.is_empty() {
            out = crate::parser::replace_table_ident(&out, "pg_publication", &values_pub(&names));
        }
    }
    if out == sql { None } else { Some(out) }
}

pub(crate) fn create(conn: &Connection, name: &str) -> Result<()> {
    let name = name.to_ascii_lowercase();
    with_session_reentrant(conn, |session| {
        session.pg_publications.insert(name);
        Ok(())
    })
}

pub(crate) fn drop(conn: &Connection, name: &str, if_exists: bool) -> Result<()> {
    let name = name.to_ascii_lowercase();
    with_session_reentrant(conn, |session| {
        if !session.pg_publications.remove(&name) && !if_exists {
            return Err(Error::UnsupportedSql(format!(
                "publication \"{name}\" does not exist"
            )));
        }
        Ok(())
    })
}

pub(crate) fn lock_done() -> Result<ExecutionResult> {
    Ok(done())
}

pub(crate) fn create_done(conn: &Connection, name: &str) -> Result<ExecutionResult> {
    create(conn, name)?;
    Ok(done())
}

pub(crate) fn drop_done(conn: &Connection, name: &str, if_exists: bool) -> Result<ExecutionResult> {
    drop(conn, name, if_exists)?;
    Ok(done())
}

pub(crate) fn snapshot_tx(session: &mut SessionState) {
    session.pg_publications_tx_snapshot = Some(session.pg_publications.clone());
}

pub(crate) fn restore_tx(session: &mut SessionState) {
    if let Some(snapshot) = session.pg_publications_tx_snapshot.take() {
        session.pg_publications = snapshot;
    }
}

pub(crate) fn restore_tx_keep(session: &mut SessionState) {
    if let Some(snapshot) = session.pg_publications_tx_snapshot.as_ref() {
        session.pg_publications = snapshot.clone();
    }
}

fn values_pub(names: &std::collections::BTreeSet<String>) -> String {
    let mut out = String::from("(VALUES ");
    for (idx, name) in names.iter().enumerate() {
        if idx > 0 {
            out.push_str(", ");
        }
        out.push_str("('");
        out.push_str(&name.replace('\'', "''"));
        out.push_str("')");
    }
    out.push_str(") AS pg_publication(pubname)");
    out
}

fn reads_from(sql: &str, name: &str) -> bool {
    let lower = sql.to_ascii_lowercase();
    let mut needle = String::from(" from ");
    needle.push_str(name);
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

fn replace_ci(sql: &str, needle: &str, replacement: &str) -> String {
    let lower = sql.to_ascii_lowercase();
    let Some(at) = lower.find(needle) else {
        return sql.to_owned();
    };
    let mut out = String::new();
    out.push_str(&sql[..at]);
    out.push_str(replacement);
    out.push_str(&sql[at + needle.len()..]);
    out
}

fn done() -> ExecutionResult {
    ExecutionResult {
        runtime: RuntimeState::Done,
        affected_rows: 0,
    }
}

fn template(conn: &Connection, sql: &str, kind: PreparedKind) -> PreparedTemplate {
    crate::parser::templates::template(sql, conn.schema_epoch(), false, kind)
}

fn plain_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
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
