//! Session `LISTEN` / `UNLISTEN` and the `pg_listening_channels()` read.
//!
//! Channels are connection-local. A transaction snapshots the set at
//! `BEGIN` and restores it on `ROLLBACK`, matching Postgres: a listen
//! that has not committed is gone after rollback.

use std::sync::Arc;

use sqlparser::ast::Ident;

use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::session::SessionState;
use crate::value::postgres_result_dialect;

pub(crate) fn channel_from_ident(ident: &Ident) -> Result<Arc<str>> {
    if ident.quote_style.is_none() && ident.value.eq_ignore_ascii_case("all") {
        return Err(Error::UnsupportedSql(
            "syntax error at or near \"ALL\"".to_owned(),
        ));
    }
    let name = if ident.quote_style.is_none() {
        ident.value.to_ascii_lowercase()
    } else {
        ident.value.clone()
    };
    if name.is_empty() {
        return Err(Error::UnsupportedSql(
            "LISTEN requires a channel name".to_owned(),
        ));
    }
    Ok(Arc::from(name))
}

pub(crate) fn listen(session: &mut SessionState, channel: &str) {
    session.pg_listening.insert(channel.to_owned());
}

pub(crate) fn unlisten(session: &mut SessionState, channel: &str) {
    if channel == "*" {
        session.pg_listening.clear();
    } else {
        session.pg_listening.remove(channel);
    }
}

pub(crate) fn snapshot_tx(session: &mut SessionState) {
    session.pg_listening_tx_snapshot = Some(session.pg_listening.clone());
}

pub(crate) fn release_tx(session: &mut SessionState) {
    session.pg_listening_tx_snapshot = None;
}

pub(crate) fn restore_tx(session: &mut SessionState) {
    if let Some(snapshot) = session.pg_listening_tx_snapshot.take() {
        session.pg_listening = snapshot;
    }
}

pub(crate) fn restore_tx_keep(session: &mut SessionState) {
    if let Some(snapshot) = session.pg_listening_tx_snapshot.as_ref() {
        session.pg_listening = snapshot.clone();
    }
}

/// Rewrite `SELECT pg_listening_channels()` into a one-column `FROM` source.
/// Scalar set-returning functions are not executed; the shell corpus calls
/// this function in the select list and in `count(*)` subqueries.
pub(crate) fn rewrite_pg_listening_channels(conn: &Connection, sql: &str) -> Option<String> {
    if !postgres_result_dialect() || !contains_call(sql) {
        return None;
    }
    let channels = crate::exec::with_session_reentrant(conn, |session| {
        Ok(session.pg_listening.iter().cloned().collect::<Vec<_>>())
    })
    .ok()?;
    let source = listening_from(&channels);
    let bytes = sql.as_bytes();
    let lower = sql.to_ascii_lowercase();
    let needle = "pg_listening_channels";
    let mut out = String::with_capacity(sql.len() + source.len());
    let mut i = 0usize;
    let mut changed = false;
    while let Some(rel) = lower[i..].find(needle) {
        let abs = i + rel;
        let after = abs + needle.len();
        let ident_prefixed = abs > 0 && is_ident_byte(bytes[abs - 1]);
        let mut j = after;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        let is_call = !ident_prefixed && j < bytes.len() && bytes[j] == b'(';
        let mut k = j + 1;
        while is_call && k < bytes.len() && bytes[k].is_ascii_whitespace() {
            k += 1;
        }
        let closed = is_call && k < bytes.len() && bytes[k] == b')';
        if closed && preceded_by_select(sql, abs) {
            out.push_str(&sql[i..abs]);
            out.push_str("pg_listening_channels FROM ");
            out.push_str(&source);
            i = k + 1;
            changed = true;
            continue;
        }
        out.push_str(&sql[i..after]);
        i = after;
    }
    if !changed {
        return None;
    }
    out.push_str(&sql[i..]);
    Some(out)
}

fn listening_from(channels: &[String]) -> String {
    if channels.is_empty() {
        return "(SELECT NULL AS pg_listening_channels WHERE 0) AS _pg_listening_channels"
            .to_owned();
    }
    let mut sql = String::from("(VALUES ");
    for (idx, channel) in channels.iter().enumerate() {
        if idx > 0 {
            sql.push_str(", ");
        }
        sql.push_str("('");
        sql.push_str(&channel.replace('\'', "''"));
        sql.push_str("')");
    }
    sql.push_str(") AS _pg_listening_channels(pg_listening_channels)");
    sql
}

fn contains_call(sql: &str) -> bool {
    sql_reads_listening_channels(sql)
}

/// The select-list rewrite embeds the current channel list in the prepared
/// template. `LISTEN` does not change `schema_epoch`, so that template must
/// not be reused from the statement cache.
pub(crate) fn sql_reads_listening_channels(sql: &str) -> bool {
    sql.to_ascii_lowercase().contains("pg_listening_channels")
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn preceded_by_select(sql: &str, call_at: usize) -> bool {
    let bytes = sql.as_bytes();
    let mut i = call_at;
    while i > 0 && bytes[i - 1].is_ascii_whitespace() {
        i -= 1;
    }
    if i < 6 {
        return false;
    }
    let start = i - 6;
    if !sql[start..i].eq_ignore_ascii_case("select") {
        return false;
    }
    start == 0 || !is_ident_byte(bytes[start - 1])
}
