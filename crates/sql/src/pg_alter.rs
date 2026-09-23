//! Postgres ALTER TABLE forms the SQLite parser does not accept.
//!
//! Inheritance is a read-time union. Logged-ness, statistics, storage,
//! reloptions, ownership, and clustering are session catalog fields.

use std::sync::Arc;

use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::statement::{PreparedKind, PreparedTemplate};

pub(crate) fn try_prepare(conn: &Connection, sql: &str) -> Result<Option<PreparedTemplate>> {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("alter table") {
        return Ok(None);
    }
    let known = lower.contains(" inherit ")
        || lower.contains(" no inherit ")
        || lower.contains(" set unlogged")
        || lower.contains(" set logged")
        || lower.contains(" set statistics ")
        || lower.contains(" set storage ")
        || lower.contains("autovacuum_enabled")
        || lower.contains(" owner to ")
        || lower.contains(" cluster on ")
        || lower.contains(" set without cluster");
    if !known {
        return Ok(None);
    }
    Ok(Some(crate::parser::templates::template(
        sql,
        conn.schema_epoch(),
        false,
        PreparedKind::PgAlter {
            sql: Arc::from(trimmed),
        },
    )))
}

pub(crate) fn apply(conn: &Connection, sql: &str) -> Result<()> {
    let lower = sql.to_ascii_lowercase();
    if let Some(rest) = after_alter_table(&lower, sql) {
        if let Some((child, parent)) = split_inherit(rest, false) {
            return link_child(conn, &parent, &child, true);
        }
        if let Some((child, parent)) = split_inherit(rest, true) {
            return link_child(conn, &parent, &child, false);
        }
        let table = first_ident(rest).unwrap_or_default();
        if lower.contains(" set unlogged") {
            return set_persistence(conn, &table, "u");
        }
        if lower.contains(" set logged") {
            return set_persistence(conn, &table, "p");
        }
        if let Some(stats) = after_ci(sql, "set statistics") {
            let column = column_before(sql, "set statistics");
            let n: i64 = stats
                .split_whitespace()
                .next()
                .unwrap_or("0")
                .parse()
                .unwrap_or(0);
            return set_stat(conn, &table, &column, n);
        }
        if lower.contains(" set storage ") {
            let column = column_before(sql, "set storage");
            let mode = last_ident(sql);
            return set_storage(conn, &table, &column, &mode);
        }
        if lower.contains("autovacuum_enabled") {
            let value = if lower.contains("= true") || lower.contains("=true") {
                "autovacuum_enabled=true"
            } else {
                "autovacuum_enabled=false"
            };
            return set_reloptions(conn, &table, value);
        }
        if lower.contains(" owner to ") {
            return set_owner(conn, &table, crate::pg_schema::SESSION_ROLE);
        }
        if lower.contains(" cluster on ") {
            let index = last_ident(sql);
            return set_cluster(conn, &table, Some(&index));
        }
        if lower.contains(" set without cluster") {
            return set_cluster(conn, &table, None);
        }
    }
    Err(Error::UnsupportedSql(format!(
        "unsupported ALTER TABLE: {sql}"
    )))
}

thread_local! {
    static EXPANDING: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

pub(crate) fn expanding() -> bool {
    EXPANDING.with(|cell| cell.get() > 0)
}

pub(crate) struct ExpandGuard;

impl Drop for ExpandGuard {
    fn drop(&mut self) {
        set_expanding(false);
    }
}

pub(crate) fn begin_expand() -> ExpandGuard {
    set_expanding(true);
    ExpandGuard
}

fn set_expanding(value: bool) {
    EXPANDING.with(|cell| {
        let now = cell.get();
        cell.set(if value {
            now.saturating_add(1)
        } else {
            now.saturating_sub(1)
        });
    });
}

pub(crate) fn rewrite_inherit(conn: &Connection, sql: &str) -> Option<String> {
    if expanding() {
        return None;
    }
    let links =
        crate::exec::with_session_reentrant(conn, |session| Ok(session.pg_children.clone()))
            .ok()?;
    if links.is_empty() {
        return None;
    }
    let mut out = sql.to_owned();
    let mut changed = false;
    for (parent, children) in links {
        if children.is_empty() {
            continue;
        }
        let mut union = format!("(SELECT * FROM {parent}");
        for child in children {
            union.push_str(&format!(" UNION ALL SELECT * FROM {child}"));
        }
        union.push_str(") AS __inh");
        let replaced = crate::parser::replace_table_ident(&out, &parent, &union);
        if replaced != out {
            out = replaced;
            changed = true;
        }
    }
    if changed { Some(out) } else { None }
}

pub(crate) fn rewrite_catalogs(conn: &Connection, sql: &str) -> Option<String> {
    let wants_attr = reads_from_catalog(sql, "pg_attribute");
    let wants_tables = reads_from_catalog(sql, "pg_tables");
    let wants_index = reads_from_catalog(sql, "pg_index");
    if !wants_attr && !wants_tables && !wants_index {
        return None;
    }
    let (stats, storage, owners, clustered) =
        crate::exec::with_session_reentrant(conn, |session| {
            Ok((
                session.pg_attstat.clone(),
                session.pg_attstorage.clone(),
                session.pg_tableowner.clone(),
                session.pg_clustered.clone(),
            ))
        })
        .ok()?;
    let mut out = sql.to_owned();
    if wants_attr {
        let mut keys = std::collections::BTreeSet::new();
        keys.extend(stats.keys().cloned());
        keys.extend(storage.keys().cloned());
        let mut values = String::new();
        for key in keys {
            let (table, column) = key.split_once('\u{1}').unwrap_or((key.as_str(), ""));
            let stat = stats.get(&key).copied().unwrap_or(-1);
            let store = storage.get(&key).map(String::as_str).unwrap_or("p");
            if !values.is_empty() {
                values.push_str(", ");
            }
            values.push_str(&format!(
                "('{}', '{}', {stat}, '{store}')",
                table.replace('\'', "''"),
                column.replace('\'', "''")
            ));
        }
        let sub = if values.is_empty() {
            "(SELECT NULL AS attrelid, NULL AS attname, NULL AS attstattarget, NULL AS attstorage WHERE 0) AS pg_attribute".to_owned()
        } else {
            format!(
                "(SELECT column1 AS attrelid, column2 AS attname, column3 AS attstattarget, column4 AS attstorage FROM (VALUES {values})) AS pg_attribute"
            )
        };
        out = crate::parser::replace_table_ident(&out, "pg_attribute", &sub);
    }
    if wants_tables {
        let mut values = String::new();
        for (table, owner) in &owners {
            if !values.is_empty() {
                values.push_str(", ");
            }
            values.push_str(&format!(
                "('{}', '{}')",
                table.replace('\'', "''"),
                owner.replace('\'', "''")
            ));
        }
        let sub = if values.is_empty() {
            "(SELECT NULL AS tablename, NULL AS tableowner WHERE 0) AS pg_tables".to_owned()
        } else {
            format!(
                "(SELECT column1 AS tablename, column2 AS tableowner FROM (VALUES {values})) AS pg_tables"
            )
        };
        out = crate::parser::replace_table_ident(&out, "pg_tables", &sub);
    }
    if wants_index {
        let mut values = String::new();
        for (table, index) in &clustered {
            if !values.is_empty() {
                values.push_str(", ");
            }
            values.push_str(&format!(
                "('{}', '{}', 't')",
                table.replace('\'', "''"),
                index.replace('\'', "''")
            ));
        }
        let sub = if values.is_empty() {
            "(SELECT NULL AS indrelid, NULL AS indexname, NULL AS indisclustered WHERE 0) AS pg_index"
                .to_owned()
        } else {
            format!(
                "(SELECT column1 AS indrelid, column2 AS indexname, column3 AS indisclustered FROM (VALUES {values})) AS pg_index"
            )
        };
        out = crate::parser::replace_table_ident(&out, "pg_index", &sub);
    }
    if out == sql { None } else { Some(out) }
}

fn touch(session: &mut crate::session::SessionState) {
    session.pg_alter_epoch = session.pg_alter_epoch.wrapping_add(1);
}

pub(crate) fn bypass_statement_cache(conn: &Connection) -> bool {
    crate::exec::with_session_reentrant(conn, |session| Ok(session.pg_alter_epoch > 0))
        .unwrap_or(false)
}

fn link_child(conn: &Connection, parent: &str, child: &str, link: bool) -> Result<()> {
    let parent = parent.to_ascii_lowercase();
    let child = child.to_ascii_lowercase();
    crate::exec::with_session_reentrant(conn, |session| {
        let entry = session.pg_children.entry(parent).or_default();
        if link {
            entry.insert(child);
        } else {
            entry.remove(&child);
        }
        touch(session);
        Ok(())
    })
}

fn set_persistence(conn: &Connection, table: &str, flag: &str) -> Result<()> {
    let table = table.to_ascii_lowercase();
    let flag = flag.to_owned();
    crate::exec::with_session_reentrant(conn, |session| {
        session.pg_relpersistence.insert(table, flag);
        touch(session);
        Ok(())
    })
}

fn set_stat(conn: &Connection, table: &str, column: &str, n: i64) -> Result<()> {
    let key = format!(
        "{}\u{1}{}",
        table.to_ascii_lowercase(),
        column.to_ascii_lowercase()
    );
    crate::exec::with_session_reentrant(conn, |session| {
        session.pg_attstat.insert(key, n);
        touch(session);
        Ok(())
    })
}

fn set_storage(conn: &Connection, table: &str, column: &str, mode: &str) -> Result<()> {
    let key = format!(
        "{}\u{1}{}",
        table.to_ascii_lowercase(),
        column.to_ascii_lowercase()
    );
    let mode = match mode.to_ascii_lowercase().as_str() {
        "external" => "e",
        "extended" => "x",
        "main" => "m",
        _ => "p",
    }
    .to_owned();
    crate::exec::with_session_reentrant(conn, |session| {
        session.pg_attstorage.insert(key, mode);
        touch(session);
        Ok(())
    })
}

fn set_reloptions(conn: &Connection, table: &str, value: &str) -> Result<()> {
    let table = table.to_ascii_lowercase();
    let value = value.to_owned();
    crate::exec::with_session_reentrant(conn, |session| {
        session.pg_reloptions.insert(table, value);
        touch(session);
        Ok(())
    })
}

fn set_owner(conn: &Connection, table: &str, owner: &str) -> Result<()> {
    let table = table.to_ascii_lowercase();
    let owner = owner.to_owned();
    crate::exec::with_session_reentrant(conn, |session| {
        session.pg_tableowner.insert(table, owner);
        touch(session);
        Ok(())
    })
}

fn set_cluster(conn: &Connection, table: &str, index: Option<&str>) -> Result<()> {
    let table = table.to_ascii_lowercase();
    crate::exec::with_session_reentrant(conn, |session| {
        match index {
            Some(index) => {
                session
                    .pg_clustered
                    .insert(table, index.to_ascii_lowercase());
            }
            None => {
                session.pg_clustered.remove(&table);
            }
        }
        touch(session);
        Ok(())
    })
}

fn after_alter_table<'a>(lower: &str, original: &'a str) -> Option<&'a str> {
    let prefix = "alter table ";
    if !lower.starts_with(prefix) {
        return None;
    }
    Some(original[prefix.len()..].trim_start())
}

fn split_inherit(rest: &str, no_inherit: bool) -> Option<(String, String)> {
    let lower = rest.to_ascii_lowercase();
    let needle = if no_inherit {
        " no inherit "
    } else {
        " inherit "
    };
    let pos = lower.find(needle)?;
    if !no_inherit && lower.contains(" no inherit ") {
        return None;
    }
    let child = rest[..pos].split_whitespace().next()?.to_owned();
    let parent = rest[pos + needle.len()..]
        .split_whitespace()
        .next()?
        .to_owned();
    Some((child, parent))
}

fn first_ident(rest: &str) -> Option<String> {
    rest.split_whitespace()
        .next()
        .map(|s| s.trim_matches('"').to_owned())
}

fn last_ident(sql: &str) -> String {
    sql.split_whitespace()
        .last()
        .unwrap_or("")
        .trim_matches(|c: char| c == '"' || c == ';' || c == ')')
        .to_owned()
}

fn column_before(sql: &str, marker: &str) -> String {
    let lower = sql.to_ascii_lowercase();
    let Some(pos) = lower.find(marker) else {
        return String::new();
    };
    sql[..pos]
        .split_whitespace()
        .last()
        .unwrap_or("")
        .trim_matches('"')
        .to_owned()
}

/// True when `name` is a FROM target. The alias on a rewritten subquery
/// (`AS pg_attribute`) must not match, or the next parse wraps it again.
fn reads_from_catalog(sql: &str, name: &str) -> bool {
    let lower = sql.to_ascii_lowercase();
    let needle = format!(" from {name}");
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

fn after_ci<'a>(sql: &'a str, marker: &str) -> Option<&'a str> {
    let lower = sql.to_ascii_lowercase();
    let pos = lower.find(marker)?;
    Some(sql[pos + marker.len()..].trim_start())
}
