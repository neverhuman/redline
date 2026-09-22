//! `CREATE` / `REFRESH` / `DROP` / `ALTER` `MATERIALIZED VIEW`.
//!
//! sqlparser accepts `CREATE MATERIALIZED VIEW` only as a view modifier and
//! does not accept `REFRESH` or `ALTER MATERIALIZED VIEW`. The corpus shapes
//! are small enough to recognize directly, including `WITH [NO] DATA`.

use std::sync::Arc;

use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::statement::{PreparedKind, PreparedTemplate};

use super::templates::template;

pub(crate) fn try_prepare(conn: &Connection, sql: &str) -> Result<Option<PreparedTemplate>> {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    let schema_epoch = conn.schema_epoch();
    if let Some(rest) = strip_prefix_ci(trimmed, "create materialized view") {
        let (name, query, populated) = parse_create(rest)?;
        return Ok(Some(template(
            sql,
            schema_epoch,
            false,
            PreparedKind::CreateMatView {
                name: Arc::from(name),
                query: Arc::from(query),
                populated,
            },
        )));
    }
    if let Some(rest) = strip_prefix_ci(trimmed, "refresh materialized view") {
        let (name, concurrently, no_data) = parse_refresh(rest)?;
        return Ok(Some(template(
            sql,
            schema_epoch,
            false,
            PreparedKind::RefreshMatView {
                name: Arc::from(name),
                concurrently,
                no_data,
            },
        )));
    }
    if let Some(rest) = strip_prefix_ci(trimmed, "drop materialized view") {
        let (name, if_exists, cascade) = parse_drop(rest)?;
        return Ok(Some(template(
            sql,
            schema_epoch,
            false,
            PreparedKind::DropMatView {
                name: Arc::from(name),
                if_exists,
                cascade,
            },
        )));
    }
    if let Some(rest) = strip_prefix_ci(trimmed, "alter materialized view") {
        let (from, to) = parse_rename(rest)?;
        return Ok(Some(template(
            sql,
            schema_epoch,
            false,
            PreparedKind::RenameMatView {
                from: Arc::from(from),
                to: Arc::from(to),
            },
        )));
    }
    Ok(None)
}

fn parse_create(rest: &str) -> Result<(String, String, bool)> {
    let rest = skip_words(rest.trim_start(), &["if", "not", "exists"]);
    let (name, rest) = take_ident(rest).ok_or_else(|| {
        Error::UnsupportedSql("CREATE MATERIALIZED VIEW requires a name".to_owned())
    })?;
    let rest = strip_prefix_ci(rest.trim_start(), "as").ok_or_else(|| {
        Error::UnsupportedSql("CREATE MATERIALIZED VIEW requires AS query".to_owned())
    })?;
    let mut query = rest.trim().to_owned();
    let populated = if strip_suffix_ci(&mut query, "with no data") {
        false
    } else {
        strip_suffix_ci(&mut query, "with data");
        true
    };
    let query = query.trim().to_owned();
    if query.is_empty() {
        return Err(Error::UnsupportedSql(
            "CREATE MATERIALIZED VIEW requires AS query".to_owned(),
        ));
    }
    Ok((name.to_ascii_lowercase(), query, populated))
}

fn parse_refresh(rest: &str) -> Result<(String, bool, bool)> {
    let rest = rest.trim_start();
    let concurrently = strip_prefix_ci(rest, "concurrently").is_some();
    let rest = if concurrently {
        strip_prefix_ci(rest, "concurrently").unwrap_or(rest)
    } else {
        rest
    };
    let (name, rest) = take_ident(rest.trim_start()).ok_or_else(|| {
        Error::UnsupportedSql("REFRESH MATERIALIZED VIEW requires a name".to_owned())
    })?;
    let mut tail = rest.trim().to_owned();
    let no_data = strip_suffix_ci(&mut tail, "with no data");
    if !tail.trim().is_empty() {
        return Err(Error::UnsupportedSql(
            "REFRESH MATERIALIZED VIEW options are not supported".to_owned(),
        ));
    }
    Ok((name.to_ascii_lowercase(), concurrently, no_data))
}

fn parse_drop(rest: &str) -> Result<(String, bool, bool)> {
    let if_exists = strip_prefix_ci(rest.trim_start(), "if exists").is_some();
    let rest = if if_exists {
        strip_prefix_ci(rest.trim_start(), "if exists").unwrap_or(rest)
    } else {
        rest
    };
    let (name, rest) = take_ident(rest.trim_start()).ok_or_else(|| {
        Error::UnsupportedSql("DROP MATERIALIZED VIEW requires a name".to_owned())
    })?;
    let mut tail = rest.trim().to_owned();
    let cascade = strip_suffix_ci(&mut tail, "cascade");
    let _ = strip_suffix_ci(&mut tail, "restrict");
    if !tail.trim().is_empty() {
        return Err(Error::UnsupportedSql(
            "DROP MATERIALIZED VIEW options are not supported".to_owned(),
        ));
    }
    Ok((name.to_ascii_lowercase(), if_exists, cascade))
}

fn parse_rename(rest: &str) -> Result<(String, String)> {
    let (from, rest) = take_ident(rest.trim_start()).ok_or_else(|| {
        Error::UnsupportedSql("ALTER MATERIALIZED VIEW requires a name".to_owned())
    })?;
    let rest = strip_prefix_ci(rest.trim_start(), "rename").ok_or_else(|| {
        Error::UnsupportedSql("ALTER MATERIALIZED VIEW requires RENAME TO".to_owned())
    })?;
    let rest = strip_prefix_ci(rest.trim_start(), "to").ok_or_else(|| {
        Error::UnsupportedSql("ALTER MATERIALIZED VIEW requires RENAME TO".to_owned())
    })?;
    let (to, rest) = take_ident(rest.trim_start()).ok_or_else(|| {
        Error::UnsupportedSql("ALTER MATERIALIZED VIEW requires a new name".to_owned())
    })?;
    if !rest.trim().is_empty() {
        return Err(Error::UnsupportedSql(
            "ALTER MATERIALIZED VIEW options are not supported".to_owned(),
        ));
    }
    Ok((from.to_ascii_lowercase(), to.to_ascii_lowercase()))
}

fn skip_words<'a>(mut rest: &'a str, words: &[&str]) -> &'a str {
    for word in words {
        let Some(next) = strip_prefix_ci(rest.trim_start(), word) else {
            return rest;
        };
        rest = next;
    }
    rest
}

fn strip_prefix_ci<'a>(sql: &'a str, prefix: &str) -> Option<&'a str> {
    let sql = sql.trim_start();
    if sql.len() < prefix.len() || !sql[..prefix.len()].eq_ignore_ascii_case(prefix) {
        return None;
    }
    let rest = &sql[prefix.len()..];
    if rest.is_empty() || rest.as_bytes()[0].is_ascii_whitespace() {
        Some(rest)
    } else {
        None
    }
}

fn strip_suffix_ci(sql: &mut String, suffix: &str) -> bool {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    let lower = trimmed.to_ascii_lowercase();
    if !lower.ends_with(suffix) {
        return false;
    }
    let at = trimmed.len() - suffix.len();
    if at > 0 && !trimmed.as_bytes()[at - 1].is_ascii_whitespace() {
        return false;
    }
    *sql = trimmed[..at].trim().to_owned();
    true
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
