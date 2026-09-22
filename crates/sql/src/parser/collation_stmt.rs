//! `CREATE COLLATION` / `DROP COLLATION` for the Postgres corpus.
//!
//! sqlparser's SQLite dialect does not accept these statements. The shapes
//! the corpus uses are small enough to recognize directly.

use std::sync::Arc;

use crate::Connection;
use crate::error::Result;
use crate::statement::{PreparedKind, PreparedTemplate};

use super::templates::template;

pub(crate) fn try_prepare(conn: &Connection, sql: &str) -> Result<Option<PreparedTemplate>> {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    let lower = trimmed.to_ascii_lowercase();
    let schema_epoch = conn.schema_epoch();
    if let Some(rest) = strip_prefix_ci(trimmed, "drop collation") {
        let if_exists = rest
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("if exists");
        let name = last_ident(rest);
        return Ok(Some(template(
            sql,
            schema_epoch,
            false,
            PreparedKind::DropCollation {
                name: Arc::from(name),
                if_exists,
            },
        )));
    }
    if let Some(rest) = strip_prefix_ci(trimmed, "create collation") {
        let rest = rest.trim_start();
        let name_end = rest.find([' ', '(']).unwrap_or(rest.len());
        let name = rest[..name_end].trim().trim_matches('"').to_owned();
        let level = if lower.contains("ks-level1") { 1 } else { 2 };
        return Ok(Some(template(
            sql,
            schema_epoch,
            false,
            PreparedKind::CreateCollation {
                name: Arc::from(name),
                level,
            },
        )));
    }
    Ok(None)
}

fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let bytes = text.as_bytes();
    let prefix = prefix.as_bytes();
    if bytes.len() < prefix.len() {
        return None;
    }
    if !bytes[..prefix.len()]
        .iter()
        .zip(prefix)
        .all(|(a, b)| a.eq_ignore_ascii_case(b))
    {
        return None;
    }
    if bytes.len() > prefix.len() && bytes[prefix.len()].is_ascii_alphanumeric() {
        return None;
    }
    Some(&text[prefix.len()..])
}

fn last_ident(text: &str) -> String {
    text.split_whitespace()
        .last()
        .unwrap_or("collation")
        .trim_matches('"')
        .to_owned()
}
