//! Postgres-dialect schema names and sequence ownership.
//!
//! The kernel catalog has one namespace (`main`). In the postgres result
//! dialect a registered schema qualifier is kept as part of the table name
//! (`auth_ns.users_collide`) so two schemas can hold the same table name.
//! `pg_catalog` is still stripped, because the catalog shims match the bare
//! name. SQLite sessions keep the old prefix strip.

use std::sync::Arc;

use redlinedb_kernel::catalog::{SchemaSnapshot, TableDef};

use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::session::SessionState;
use crate::statement::{PreparedKind, PreparedTemplate};
use crate::value::SqlValue;

/// Single role this shell reports for `current_user` and `nspowner` 10.
pub(crate) const SESSION_ROLE: &str = "redlinedb";

pub(crate) fn is_nextval_default(expr_sql: &str) -> bool {
    expr_sql.starts_with("nextval:")
}

pub(crate) fn try_prepare(conn: &Connection, sql: &str) -> Result<Option<PreparedTemplate>> {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    let Some(rest) = strip_prefix_ci(trimmed, "alter sequence") else {
        return Ok(None);
    };
    let (name, rest) = sequence_name(rest.trim_start())
        .ok_or_else(|| Error::UnsupportedSql("ALTER SEQUENCE requires a name".to_owned()))?;
    let Some(owned) = strip_prefix_ci(rest.trim_start(), "owned by") else {
        return Err(Error::UnsupportedSql(
            "ALTER SEQUENCE supports OWNED BY".to_owned(),
        ));
    };
    let owned = owned.trim();
    if owned.is_empty() {
        return Err(Error::UnsupportedSql(
            "ALTER SEQUENCE OWNED BY requires a column".to_owned(),
        ));
    }
    let schema_epoch = conn.schema_epoch();
    Ok(Some(crate::parser::templates::template(
        sql,
        schema_epoch,
        false,
        PreparedKind::AlterSequenceOwned {
            name: Arc::from(name),
            owned_by: Arc::from(owned.to_ascii_lowercase()),
        },
    )))
}

pub(crate) fn is_sequence_ddl(sql: &str) -> bool {
    let trimmed = sql.trim_start();
    starts_with_keyword(trimmed, "create sequence")
        || starts_with_keyword(trimmed, "drop sequence")
        || starts_with_keyword(trimmed, "alter sequence")
}

/// `current_user` without parentheses is a function, except after `AUTHORIZATION`.
pub(crate) fn rewrite_session_keywords(sql: &str) -> Option<String> {
    let lower = sql.to_ascii_lowercase();
    if !lower.contains("current_user")
        && !lower.contains("session_user")
        && !lower.contains("current_role")
    {
        return None;
    }
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0usize;
    let mut last = 0usize;
    let mut changed = false;
    let mut in_str: Option<u8> = None;
    while i < bytes.len() {
        if in_str.is_none() && bytes[i] == b'$' {
            if let Some(end) = dollar_quote_end(bytes, i) {
                i = end;
                continue;
            }
        }
        let b = bytes[i];
        if let Some(quote) = in_str {
            if b == quote {
                in_str = None;
            }
            i += 1;
            continue;
        }
        if b == b'\'' || b == b'"' {
            in_str = Some(b);
            i += 1;
            continue;
        }
        let ident_start = b.is_ascii_alphabetic() || b == b'_';
        let prev_word = i > 0
            && (bytes[i - 1].is_ascii_alphanumeric()
                || bytes[i - 1] == b'_'
                || bytes[i - 1] == b'.');
        if !ident_start || prev_word {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
            j += 1;
        }
        let ident = sql[i..j].to_ascii_lowercase();
        let keyword = matches!(
            ident.as_str(),
            "current_user" | "session_user" | "current_role"
        );
        let next_call = j < bytes.len() && bytes[j] == b'(';
        if keyword && !next_call && last_word(&sql[..i]).to_ascii_lowercase() != "authorization" {
            out.push_str(&sql[last..j]);
            out.push_str("()");
            last = j;
            changed = true;
        }
        i = j;
    }
    if !changed {
        return None;
    }
    out.push_str(&sql[last..]);
    Some(out)
}

/// Rewrite schema-qualified table names for the postgres dialect.
/// `None` means the statement is unchanged.
pub(crate) fn rewrite_tables(conn: &Connection, sql: &str) -> Option<String> {
    if !sql.contains('.') && !contains_relation_keyword(sql) {
        return None;
    }
    let schemas =
        crate::exec::with_session_reentrant(conn, |session| Ok(session.pg_schemas.clone())).ok()?;
    if schemas.is_empty() {
        return None;
    }
    let path = crate::exec::with_session_reentrant(conn, |session| Ok(session.search_path.clone()))
        .unwrap_or_else(|_| "\"$user\", public".to_owned());
    let snapshot = conn.schema_snapshot();
    let path = search_schemas(&path);
    encode_schema_names(sql, &schemas, &snapshot, &path)
}

pub(crate) fn fill_nextval(
    session: &mut SessionState,
    table: &TableDef,
    provided: &[usize],
    values: &mut [SqlValue],
) -> Result<()> {
    for (idx, column) in table.columns.iter().enumerate() {
        if provided.iter().any(|ordinal| *ordinal == idx) {
            continue;
        }
        let Some(generated) = &column.generated else {
            continue;
        };
        let Some(sequence) = generated.expr_sql.strip_prefix("nextval:") else {
            continue;
        };
        let entry = session.pg_sequences.get_mut(sequence).ok_or_else(|| {
            Error::UnsupportedSql(format!("relation \"{sequence}\" does not exist"))
        })?;
        let next = match entry.last_value {
            Some(value) => value + entry.increment,
            None => entry.start,
        };
        entry.last_value = Some(next);
        if let Some(slot) = values.get_mut(idx) {
            *slot = SqlValue::Integer(next);
        }
    }
    Ok(())
}

pub(crate) fn sequence_key(name: &str) -> String {
    let folded = name.to_ascii_lowercase();
    folded
        .rsplit_once('.')
        .map(|(_, bare)| bare.to_owned())
        .unwrap_or(folded)
}

fn encode_schema_names(
    sql: &str,
    schemas: &std::collections::BTreeSet<String>,
    snapshot: &SchemaSnapshot,
    path: &[String],
) -> Option<String> {
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0usize;
    let mut last = 0usize;
    let mut changed = false;
    let mut in_str: Option<u8> = None;
    while i < bytes.len() {
        if in_str.is_none() && bytes[i] == b'$' {
            if let Some(end) = dollar_quote_end(bytes, i) {
                i = end;
                continue;
            }
        }
        let b = bytes[i];
        if let Some(quote) = in_str {
            if b == quote {
                if quote == b'\'' && i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    i += 2;
                    continue;
                }
                in_str = None;
            }
            i += 1;
            continue;
        }
        if b == b'\'' || b == b'"' {
            in_str = Some(b);
            i += 1;
            continue;
        }
        let ident_start = b.is_ascii_alphabetic() || b == b'_';
        let prev_word = i > 0
            && (bytes[i - 1].is_ascii_alphanumeric()
                || bytes[i - 1] == b'_'
                || bytes[i - 1] == b'.'
                || bytes[i - 1] == b'"');
        if !ident_start || prev_word {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b'.' {
            let schema = sql[i..j].to_ascii_lowercase();
            if schemas.contains(&schema) {
                let mut k = j + 1;
                if k < bytes.len() && (bytes[k].is_ascii_alphabetic() || bytes[k] == b'_') {
                    let name_at = k;
                    k += 1;
                    while k < bytes.len() && (bytes[k].is_ascii_alphanumeric() || bytes[k] == b'_')
                    {
                        k += 1;
                    }
                    let call = k < bytes.len() && bytes[k] == b'(';
                    if schema == "pg_catalog" && !call {
                        out.push_str(&sql[last..i]);
                        last = j + 1;
                        i = j + 1;
                        changed = true;
                        continue;
                    }
                    if schema != "pg_catalog" && !call {
                        out.push_str(&sql[last..i]);
                        out.push('"');
                        out.push_str(&sql[i..j]);
                        out.push('.');
                        out.push_str(&sql[name_at..k]);
                        out.push('"');
                        last = k;
                        i = k;
                        changed = true;
                        continue;
                    }
                }
            }
            i = j;
            continue;
        }
        if relation_keyword_before(&sql[..i]) {
            let ident = &sql[i..j];
            if let Some(encoded) = resolve_unqualified(snapshot, path, ident) {
                out.push_str(&sql[last..i]);
                out.push('"');
                out.push_str(&encoded);
                out.push('"');
                last = j;
                i = j;
                changed = true;
                continue;
            }
        }
        i = j;
    }
    if !changed {
        return None;
    }
    out.push_str(&sql[last..]);
    Some(out)
}

fn resolve_unqualified(snapshot: &SchemaSnapshot, path: &[String], ident: &str) -> Option<String> {
    let folded = ident.to_ascii_lowercase();
    if table_exists(snapshot, &folded) {
        return None;
    }
    for schema in path {
        let encoded = format!("{schema}.{folded}");
        if table_exists(snapshot, &encoded) {
            return Some(encoded);
        }
    }
    None
}

fn table_exists(snapshot: &SchemaSnapshot, name: &str) -> bool {
    let Some(schema_id) = snapshot.lookup_namespace("main") else {
        return false;
    };
    snapshot.lookup_table(schema_id, name).is_some()
}

fn search_schemas(path: &str) -> Vec<String> {
    if path == "\"\"" {
        return Vec::new();
    }
    path.split(',')
        .filter_map(|part| {
            let name = part.trim().trim_matches('"');
            if name.is_empty() || name == "$user" {
                None
            } else {
                Some(name.to_ascii_lowercase())
            }
        })
        .collect()
}

fn relation_keyword_before(prefix: &str) -> bool {
    matches!(
        last_word(prefix).to_ascii_lowercase().as_str(),
        "from" | "join" | "into" | "update" | "table"
    )
}

fn contains_relation_keyword(sql: &str) -> bool {
    let lower = sql.to_ascii_lowercase();
    [" from ", " join ", " into ", " update ", " table "]
        .iter()
        .any(|needle| lower.contains(needle))
}

fn last_word(sql: &str) -> &str {
    let trimmed = sql.trim_end();
    let bytes = trimmed.as_bytes();
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
        start -= 1;
    }
    &trimmed[start..end]
}

fn sequence_name(rest: &str) -> Option<(String, &str)> {
    let (schema, after_schema) = take_ident(rest)?;
    let after_schema = after_schema.trim_start();
    if let Some(after_dot) = after_schema.strip_prefix('.') {
        let (name, after_name) = take_ident(after_dot.trim_start())?;
        Some((name.to_ascii_lowercase(), after_name))
    } else {
        Some((schema.to_ascii_lowercase(), after_schema))
    }
}

fn take_ident(rest: &str) -> Option<(&str, &str)> {
    let mut end = 0usize;
    for (idx, ch) in rest.char_indices() {
        if idx == 0 {
            if !(ch.is_ascii_alphabetic() || ch == '_') {
                return None;
            }
        } else if !(ch.is_ascii_alphanumeric() || ch == '_') {
            break;
        }
        end = idx + ch.len_utf8();
    }
    if end == 0 {
        return None;
    }
    Some((&rest[..end], &rest[end..]))
}

fn starts_with_keyword(sql: &str, keyword: &str) -> bool {
    let Some(rest) = strip_prefix_ci(sql, keyword) else {
        return false;
    };
    rest.is_empty() || rest.as_bytes()[0].is_ascii_whitespace()
}

fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let mut text_chars = text.chars();
    for expected in prefix.chars() {
        let found = text_chars.next()?;
        if !found.eq_ignore_ascii_case(&expected) {
            return None;
        }
    }
    Some(text_chars.as_str())
}

fn dollar_quote_end(bytes: &[u8], start: usize) -> Option<usize> {
    if start >= bytes.len() || bytes[start] != b'$' {
        return None;
    }
    let mut tag_end = start + 1;
    if tag_end < bytes.len() && bytes[tag_end] == b'$' {
        let mut k = tag_end + 1;
        while k + 1 < bytes.len() {
            if bytes[k] == b'$' && bytes[k + 1] == b'$' {
                return Some(k + 2);
            }
            k += 1;
        }
        return None;
    }
    while tag_end < bytes.len()
        && (bytes[tag_end].is_ascii_alphanumeric() || bytes[tag_end] == b'_')
    {
        tag_end += 1;
    }
    if tag_end == start + 1 || tag_end >= bytes.len() || bytes[tag_end] != b'$' {
        return None;
    }
    let tag = &bytes[start..=tag_end];
    let mut k = tag_end + 1;
    while k + tag.len() <= bytes.len() {
        if &bytes[k..k + tag.len()] == tag {
            return Some(k + tag.len());
        }
        k += 1;
    }
    None
}
