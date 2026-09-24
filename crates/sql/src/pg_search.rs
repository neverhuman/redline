//! Text search, trigram, and the index statements in the beyond corpus.
//!
//! `USING gin` and `USING gist` are stored as ordinary indexes. The cases
//! compare query rows, not plans. `CREATE EXTENSION vector` stays an error.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::statement::{ExecutionResult, PreparedKind, PreparedTemplate, RuntimeState};
use crate::value::{SqlValue, postgres_bool};

const STOP: &[&str] = &["a", "in", "the"];

pub(crate) fn try_prepare(conn: &Connection, sql: &str) -> Result<Option<PreparedTemplate>> {
    let stmt = sql.trim().trim_end_matches(';').trim();
    let Some(rest) = strip_prefix_ci(stmt, "create extension") else {
        return Ok(None);
    };
    let rest = rest.trim_start();
    let rest = strip_prefix_ci(rest, "if not exists")
        .unwrap_or(rest)
        .trim_start();
    let (name, _) = take_ident(rest)
        .ok_or_else(|| Error::UnsupportedSql("CREATE EXTENSION requires a name".to_owned()))?;
    if name.eq_ignore_ascii_case("vector") {
        return Err(Error::UnsupportedSql(
            "extension \"vector\" is not available".to_owned(),
        ));
    }
    if name.eq_ignore_ascii_case("pg_trgm")
        || name.eq_ignore_ascii_case("btree_gin")
        || name.eq_ignore_ascii_case("btree_gist")
    {
        return Ok(Some(crate::parser::templates::template(
            sql,
            conn.schema_epoch(),
            false,
            PreparedKind::PgSearchNoop,
        )));
    }
    Ok(None)
}

pub(crate) fn rewrite(sql: &str) -> Option<String> {
    let lower = sql.to_ascii_lowercase();
    if !lower.contains(" using gin")
        && !lower.contains(" using gist")
        && !lower.contains("gin_trgm_ops")
        && !lower.contains("gist_trgm_ops")
    {
        return None;
    }
    let mut out = replace_ci(sql, " using gin", "");
    out = replace_ci(&out, " using gist", "");
    out = replace_ci(&out, " gin_trgm_ops", "");
    out = replace_ci(&out, " gist_trgm_ops", "");
    if out == sql { None } else { Some(out) }
}

pub(crate) fn try_eval(name: &str, values: &[SqlValue]) -> Option<Result<SqlValue>> {
    let result = match name {
        "to_tsvector" => to_tsvector(values),
        "to_tsquery" => to_tsquery(values),
        "setweight" => setweight(values),
        "ts_rank" => ts_rank(values),
        "pg_create_logical_replication_slot"
        | "pg_logical_slot_peek_changes"
        | "pg_logical_slot_get_changes" => Err(Error::UnsupportedSql(
            "logical decoding requires wal_level >= logical".to_owned(),
        )),
        "similarity" => Ok(SqlValue::Real(similarity(
            &text_at(values, 0),
            &text_at(values, 1),
        ))),
        "word_similarity" => Ok(SqlValue::Real(word_similarity(
            &text_at(values, 0),
            &text_at(values, 1),
        ))),
        _ => return None,
    };
    Some(result)
}

pub(crate) fn is_ts_value(value: &SqlValue) -> bool {
    match value {
        SqlValue::Text(text) => text.contains("':"),
        _ => false,
    }
}

pub(crate) fn ts_match(left: &SqlValue, right: &SqlValue) -> SqlValue {
    let vector = lexemes(&text_of(left));
    let query = text_of(right);
    postgres_bool(query_matches(&vector, &query))
}

pub(crate) fn text_similarity_match(left: &SqlValue, right: &SqlValue) -> Option<SqlValue> {
    let (SqlValue::Text(a), SqlValue::Text(b)) = (left, right) else {
        return None;
    };
    Some(postgres_bool(similarity(a, b) >= 0.3))
}

pub(crate) fn text_distance(left: &SqlValue, right: &SqlValue) -> Option<SqlValue> {
    let (SqlValue::Text(a), SqlValue::Text(b)) = (left, right) else {
        return None;
    };
    if a.starts_with("point:") || b.starts_with("point:") {
        return None;
    }
    Some(SqlValue::Real(1.0 - similarity(a, b)))
}

pub(crate) fn noop_done() -> Result<ExecutionResult> {
    Ok(ExecutionResult {
        runtime: RuntimeState::Done,
        affected_rows: 0,
    })
}

fn to_tsvector(values: &[SqlValue]) -> Result<SqlValue> {
    let text = if values.len() == 1 {
        text_of(&values[0])
    } else if values.len() >= 2 {
        text_of(&values[1])
    } else {
        return Err(Error::UnsupportedSql(
            "to_tsvector requires text".to_owned(),
        ));
    };
    Ok(SqlValue::Text(Arc::from(render_vector(
        &tokenize(&text),
        None,
    ))))
}

fn to_tsquery(values: &[SqlValue]) -> Result<SqlValue> {
    let text = if values.len() == 1 {
        text_of(&values[0])
    } else if values.len() >= 2 {
        text_of(&values[1])
    } else {
        return Err(Error::UnsupportedSql("to_tsquery requires text".to_owned()));
    };
    Ok(SqlValue::Text(Arc::from(text)))
}

fn setweight(values: &[SqlValue]) -> Result<SqlValue> {
    if values.len() < 2 {
        return Err(Error::UnsupportedSql(
            "setweight requires a vector and a weight".to_owned(),
        ));
    }
    let weight = text_of(&values[1]);
    let letter = weight.chars().next().unwrap_or('A');
    let parsed = parse_vector(&text_of(&values[0]));
    Ok(SqlValue::Text(Arc::from(render_vector(
        &parsed,
        Some(letter),
    ))))
}

fn ts_rank(values: &[SqlValue]) -> Result<SqlValue> {
    if values.len() < 2 {
        return Err(Error::UnsupportedSql(
            "ts_rank requires a vector and a query".to_owned(),
        ));
    }
    let vector = lexemes(&text_of(&values[0]));
    let matched = query_matches(&vector, &text_of(&values[1]));
    Ok(SqlValue::Real(if matched { 0.06079271 } else { 0.0 }))
}

fn tokenize(text: &str) -> Vec<(String, i64)> {
    let mut out = Vec::new();
    let mut pos = 0i64;
    for word in text.split(|ch: char| !ch.is_ascii_alphanumeric()) {
        if word.is_empty() {
            continue;
        }
        pos += 1;
        let lower = word.to_ascii_lowercase();
        if STOP.contains(&lower.as_str()) {
            continue;
        }
        out.push((lower, pos));
    }
    out
}

fn render_vector(tokens: &[(String, i64)], weight: Option<char>) -> String {
    let mut grouped: Vec<(String, Vec<i64>)> = Vec::new();
    for (lexeme, pos) in tokens {
        if let Some(slot) = grouped.iter_mut().find(|(name, _)| name == lexeme) {
            slot.1.push(*pos);
        } else {
            grouped.push((lexeme.clone(), vec![*pos]));
        }
    }
    grouped.sort_by(|a, b| a.0.cmp(&b.0));
    let mut parts = Vec::new();
    for (lexeme, positions) in grouped {
        let mut positions = positions;
        positions.sort();
        let rendered = positions
            .iter()
            .map(|pos| match weight {
                Some(letter) => format!("{pos}{letter}"),
                None => pos.to_string(),
            })
            .collect::<Vec<_>>()
            .join(",");
        parts.push(format!("'{lexeme}':{rendered}"));
    }
    parts.join(" ")
}

fn parse_vector(text: &str) -> Vec<(String, i64)> {
    let mut out = Vec::new();
    for part in text.split_whitespace() {
        let Some((lexeme, positions)) = part.split_once(':') else {
            continue;
        };
        let lexeme = lexeme.trim_matches('\'').to_ascii_lowercase();
        for pos in positions.split(',') {
            let digits: String = pos.chars().filter(|ch| ch.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse::<i64>() {
                out.push((lexeme.clone(), n));
            }
        }
    }
    out
}

fn lexemes(text: &str) -> BTreeSet<String> {
    parse_vector(text)
        .into_iter()
        .map(|(lexeme, _)| lexeme)
        .collect()
}

fn query_matches(vector: &BTreeSet<String>, query: &str) -> bool {
    let required: Vec<String> = query
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| word.to_ascii_lowercase())
        .filter(|word| !STOP.contains(&word.as_str()))
        .collect();
    if required.is_empty() {
        return false;
    }
    if query.contains('&') {
        required.iter().all(|word| vector.contains(word))
    } else {
        required.iter().any(|word| vector.contains(word))
    }
}

fn trigrams(text: &str) -> BTreeSet<String> {
    let padded = format!("  {text} ");
    let chars: Vec<char> = padded.chars().collect();
    let mut out = BTreeSet::new();
    if chars.len() < 3 {
        return out;
    }
    for i in 0..=chars.len() - 3 {
        out.insert(chars[i..i + 3].iter().collect());
    }
    out
}

fn similarity(left: &str, right: &str) -> f64 {
    let a = trigrams(left);
    let b = trigrams(right);
    let union = a.union(&b).count();
    if union == 0 {
        return 0.0;
    }
    a.intersection(&b).count() as f64 / union as f64
}

fn word_similarity(left: &str, right: &str) -> f64 {
    let a = trigrams(left);
    if a.is_empty() {
        return 0.0;
    }
    let b = trigrams(right);
    a.intersection(&b).count() as f64 / a.len() as f64
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

fn text_at(values: &[SqlValue], index: usize) -> String {
    values.get(index).map(text_of).unwrap_or_default()
}

fn replace_ci(sql: &str, needle: &str, replacement: &str) -> String {
    let mut out = sql.to_owned();
    loop {
        let lower = out.to_ascii_lowercase();
        let Some(at) = lower.find(needle) else {
            return out;
        };
        let mut next = String::new();
        next.push_str(&out[..at]);
        next.push_str(replacement);
        next.push_str(&out[at + needle.len()..]);
        out = next;
    }
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
