//! Postgres enum and domain types for the result dialect.
//!
//! Enum order is declaration order. A domain cast applies its check and
//! then returns the base value. SQLite sessions do not consult these maps.

use std::sync::Arc;

use sqlparser::ast::{BinaryOperator, Expr};

use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::exec::expr::{RowContext, eval_scalar};
use crate::statement::{PreparedKind, PreparedTemplate};
use crate::value::SqlValue;

#[derive(Clone, Debug)]
pub(crate) struct PgDomain {
    /// Reject values that are not strictly greater than this integer.
    pub greater_than: i64,
}

pub(crate) fn try_prepare(conn: &Connection, sql: &str) -> Result<Option<PreparedTemplate>> {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    let schema_epoch = conn.schema_epoch();
    if let Some(rest) = strip_prefix_ci(trimmed, "drop extension") {
        let (if_exists, name) = drop_name(rest);
        if name.eq_ignore_ascii_case("citext") {
            return Ok(Some(crate::parser::templates::template(
                sql,
                schema_epoch,
                false,
                PreparedKind::SetPgCitext { enabled: false },
            )));
        }
        let _ = if_exists;
        return Ok(None);
    }
    if let Some(rest) = strip_prefix_ci(trimmed, "create extension") {
        let rest = rest.trim_start();
        let rest = strip_prefix_ci(rest, "if not exists")
            .unwrap_or(rest)
            .trim_start();
        let (name, _) = take_ident(rest)
            .ok_or_else(|| Error::UnsupportedSql("CREATE EXTENSION requires a name".to_owned()))?;
        if name.eq_ignore_ascii_case("citext") {
            return Ok(Some(crate::parser::templates::template(
                sql,
                schema_epoch,
                false,
                PreparedKind::SetPgCitext { enabled: true },
            )));
        }
        return Ok(None);
    }
    if let Some(rest) = strip_prefix_ci(trimmed, "drop type") {
        let (if_exists, name) = drop_name(rest);
        return Ok(Some(crate::parser::templates::template(
            sql,
            schema_epoch,
            false,
            PreparedKind::DropPgEnum {
                name: Arc::from(name),
                if_exists,
            },
        )));
    }
    if let Some(rest) = strip_prefix_ci(trimmed, "drop domain") {
        let (if_exists, name) = drop_name(rest);
        return Ok(Some(crate::parser::templates::template(
            sql,
            schema_epoch,
            false,
            PreparedKind::DropPgDomain {
                name: Arc::from(name),
                if_exists,
            },
        )));
    }
    if let Some(rest) = strip_prefix_ci(trimmed, "create type") {
        let (name, rest) = take_ident(rest.trim_start())
            .ok_or_else(|| Error::UnsupportedSql("CREATE TYPE requires a name".to_owned()))?;
        let rest = strip_prefix_ci(rest.trim_start(), "as enum")
            .ok_or_else(|| Error::UnsupportedSql("CREATE TYPE supports AS ENUM".to_owned()))?;
        let labels = enum_labels(rest.trim_start())?;
        return Ok(Some(crate::parser::templates::template(
            sql,
            schema_epoch,
            false,
            PreparedKind::CreatePgEnum {
                name: Arc::from(name.to_ascii_lowercase()),
                labels: Arc::from(labels),
            },
        )));
    }
    if let Some(rest) = strip_prefix_ci(trimmed, "create domain") {
        let (name, rest) = take_ident(rest.trim_start())
            .ok_or_else(|| Error::UnsupportedSql("CREATE DOMAIN requires a name".to_owned()))?;
        let rest = strip_prefix_ci(rest.trim_start(), "as")
            .ok_or_else(|| Error::UnsupportedSql("CREATE DOMAIN requires AS".to_owned()))?;
        let greater_than = domain_greater_than(rest)?;
        return Ok(Some(crate::parser::templates::template(
            sql,
            schema_epoch,
            false,
            PreparedKind::CreatePgDomain {
                name: Arc::from(name.to_ascii_lowercase()),
                greater_than,
            },
        )));
    }
    Ok(None)
}

pub(crate) fn create_enum(conn: &Connection, name: &str, labels: &[String]) -> Result<()> {
    crate::exec::with_session_reentrant(conn, |session| {
        session
            .pg_enums
            .insert(name.to_ascii_lowercase(), labels.to_vec());
        Ok(())
    })
}

pub(crate) fn drop_enum(conn: &Connection, name: &str, if_exists: bool) -> Result<()> {
    let key = name.to_ascii_lowercase();
    crate::exec::with_session_reentrant(conn, |session| {
        if session.pg_enums.remove(&key).is_none() && !if_exists {
            return Err(Error::UnsupportedSql(format!(
                "type \"{name}\" does not exist"
            )));
        }
        Ok(())
    })
}

pub(crate) fn create_domain(conn: &Connection, name: &str, greater_than: i64) -> Result<()> {
    crate::exec::with_session_reentrant(conn, |session| {
        session
            .pg_domains
            .insert(name.to_ascii_lowercase(), PgDomain { greater_than });
        Ok(())
    })
}

pub(crate) fn drop_domain(conn: &Connection, name: &str, if_exists: bool) -> Result<()> {
    let key = name.to_ascii_lowercase();
    crate::exec::with_session_reentrant(conn, |session| {
        if session.pg_domains.remove(&key).is_none() && !if_exists {
            return Err(Error::UnsupportedSql(format!(
                "domain \"{name}\" does not exist"
            )));
        }
        Ok(())
    })
}

pub(crate) fn is_registered(type_name: &str) -> bool {
    if !crate::value::postgres_result_dialect() {
        return false;
    }
    let Some(conn) = crate::exec::current_connection() else {
        return false;
    };
    let key = type_name.to_ascii_lowercase();
    crate::exec::with_session_reentrant(conn, |session| {
        Ok(session.pg_enums.contains_key(&key)
            || session.pg_domains.contains_key(&key)
            || (key == "citext" && session.pg_citext))
    })
    .unwrap_or(false)
}

pub(crate) const CITEXT_MARK: char = '\u{E000}';

pub(crate) fn set_citext(conn: &Connection, enabled: bool) -> Result<()> {
    crate::exec::with_session_reentrant(conn, |session| {
        session.pg_citext = enabled;
        Ok(())
    })
}

pub(crate) fn int4range_value(lo: i64, hi: i64) -> SqlValue {
    SqlValue::Text(Arc::from(format!("range:{lo}:{hi}")))
}

pub(crate) fn parse_range(value: &SqlValue) -> Option<(i64, i64)> {
    let SqlValue::Text(text) = value else {
        return None;
    };
    if let Some(rest) = text.strip_prefix("range:") {
        let (lo, hi) = rest.split_once(':')?;
        return Some((lo.parse().ok()?, hi.parse().ok()?));
    }
    let trimmed = text.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(')') {
        let inner = &trimmed[1..trimmed.len() - 1];
        let (lo, hi) = inner.split_once(',')?;
        return Some((lo.trim().parse().ok()?, hi.trim().parse().ok()?));
    }
    None
}

pub(crate) fn range_contains(range: &SqlValue, value: &SqlValue) -> Option<bool> {
    let (lo, hi) = parse_range(range)?;
    let n = match value {
        SqlValue::Integer(n) => *n,
        SqlValue::Text(text) => text.parse().ok()?,
        _ => return None,
    };
    Some(n >= lo && n < hi)
}

pub(crate) fn range_overlaps(left: &SqlValue, right: &SqlValue) -> Option<bool> {
    let (a, b) = parse_range(left)?;
    let (c, d) = parse_range(right)?;
    Some(a < d && c < b)
}

pub(crate) fn point_value(x: f64, y: f64) -> SqlValue {
    SqlValue::Text(Arc::from(format!("point:{x}:{y}")))
}

pub(crate) fn parse_point(value: &SqlValue) -> Option<(f64, f64)> {
    let SqlValue::Text(text) = value else {
        return None;
    };
    let rest = text.strip_prefix("point:")?;
    let (x, y) = rest.split_once(':')?;
    Some((x.parse().ok()?, y.parse().ok()?))
}

pub(crate) fn point_distance(left: &SqlValue, right: &SqlValue) -> Option<SqlValue> {
    let (x1, y1) = parse_point(left)?;
    let (x2, y2) = parse_point(right)?;
    let d = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt();
    if (d - d.round()).abs() < 1e-9 {
        Some(SqlValue::Integer(d.round() as i64))
    } else {
        Some(SqlValue::Real(d))
    }
}

pub(crate) fn as_number(value: &SqlValue) -> Option<f64> {
    match value {
        SqlValue::Integer(n) => Some(*n as f64),
        SqlValue::Real(n) => Some(*n),
        SqlValue::Text(text) => text.parse().ok(),
        _ => None,
    }
}

/// `Some` when `type_name` is a registered enum or domain.
pub(crate) fn cast_registered(type_name: &str, value: &SqlValue) -> Result<Option<SqlValue>> {
    if type_name.eq_ignore_ascii_case("int4range") {
        if let Some((lo, hi)) = parse_range(value) {
            return Ok(Some(int4range_value(lo, hi)));
        }
    }
    if !crate::value::postgres_result_dialect() {
        return Ok(None);
    }
    let Some(conn) = crate::exec::current_connection() else {
        return Ok(None);
    };
    let key = type_name.to_ascii_lowercase();
    let (labels, domain, citext) = crate::exec::with_session_reentrant(&conn, |session| {
        Ok((
            session.pg_enums.get(&key).cloned(),
            session.pg_domains.get(&key).cloned(),
            key == "citext" && session.pg_citext,
        ))
    })?;
    if citext {
        let text = match value {
            SqlValue::Text(text) => text.to_string(),
            SqlValue::Integer(n) => n.to_string(),
            SqlValue::Null => return Ok(Some(SqlValue::Null)),
            _ => return Err(Error::UnsupportedSql("cannot cast to citext".to_owned())),
        };
        return Ok(Some(SqlValue::Text(Arc::from(format!(
            "{CITEXT_MARK}{text}"
        )))));
    }
    if let Some(labels) = labels {
        let text = match value {
            SqlValue::Text(text) => text.to_string(),
            SqlValue::Integer(n) => n.to_string(),
            _ => {
                return Err(Error::UnsupportedSql(format!(
                    "invalid input value for enum {key}"
                )));
            }
        };
        if !labels.iter().any(|label| label == &text) {
            return Err(Error::UnsupportedSql(format!(
                "invalid input value for enum {key}: \"{text}\""
            )));
        }
        return Ok(Some(SqlValue::Text(Arc::from(text))));
    }
    if let Some(domain) = domain {
        let number = match value {
            SqlValue::Integer(n) => *n,
            SqlValue::Text(text) => text.parse::<i64>().map_err(|_| {
                Error::UnsupportedSql(format!(
                    "value for domain {key} violates check constraint \"{key}_check\""
                ))
            })?,
            _ => {
                return Err(Error::UnsupportedSql(format!(
                    "value for domain {key} violates check constraint \"{key}_check\""
                )));
            }
        };
        if number <= domain.greater_than {
            return Err(Error::UnsupportedSql(format!(
                "value for domain {key} violates check constraint \"{key}_check\""
            )));
        }
        return Ok(Some(SqlValue::Integer(number)));
    }
    Ok(None)
}

pub(crate) fn compare_enums(
    left: &Expr,
    op: &BinaryOperator,
    right: &Expr,
    row: &RowContext<'_>,
    bindings: &[Option<SqlValue>],
) -> Result<Option<SqlValue>> {
    let (Some(left_ty), Some(right_ty)) = (cast_type_name(left), cast_type_name(right)) else {
        return Ok(None);
    };
    if !left_ty.eq_ignore_ascii_case(&right_ty) {
        return Ok(None);
    }
    let Some(conn) = crate::exec::current_connection() else {
        return Ok(None);
    };
    let labels = crate::exec::with_session_reentrant(&conn, |session| {
        Ok(session.pg_enums.get(&left_ty.to_ascii_lowercase()).cloned())
    })?;
    let Some(labels) = labels else {
        return Ok(None);
    };
    let left_value = eval_scalar(left, row, bindings)?;
    let right_value = eval_scalar(right, row, bindings)?;
    let left_at = ordinal(&labels, &left_value);
    let right_at = ordinal(&labels, &right_value);
    let (Some(left_at), Some(right_at)) = (left_at, right_at) else {
        return Ok(None);
    };
    let ordering = left_at.cmp(&right_at);
    let result = match op {
        BinaryOperator::Lt => ordering.is_lt(),
        BinaryOperator::LtEq => ordering.is_le(),
        BinaryOperator::Gt => ordering.is_gt(),
        BinaryOperator::GtEq => ordering.is_ge(),
        BinaryOperator::Eq => ordering.is_eq(),
        BinaryOperator::NotEq => ordering.is_ne(),
        _ => return Ok(None),
    };
    Ok(Some(crate::value::postgres_bool(result)))
}

fn ordinal(labels: &[String], value: &SqlValue) -> Option<usize> {
    let text = match value {
        SqlValue::Text(text) => text.as_ref(),
        _ => return None,
    };
    labels.iter().position(|label| label == text)
}

fn cast_type_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Cast { data_type, .. } => Some(data_type.to_string()),
        _ => None,
    }
}

fn drop_name(rest: &str) -> (bool, String) {
    let rest = rest.trim_start();
    let if_exists = strip_prefix_ci(rest, "if exists").is_some();
    let body = if if_exists {
        strip_prefix_ci(rest, "if exists")
            .unwrap_or(rest)
            .trim_start()
    } else {
        rest
    };
    let name = body
        .split_whitespace()
        .next()
        .unwrap_or("type")
        .trim_matches('"')
        .to_owned();
    (if_exists, name)
}

fn enum_labels(rest: &str) -> Result<Vec<String>> {
    let rest = rest.trim().trim_start_matches('(').trim_end_matches(')');
    let mut labels = Vec::new();
    for part in rest.split(',') {
        let label = part.trim().trim_matches('\'').trim_matches('"');
        if label.is_empty() {
            return Err(Error::UnsupportedSql(
                "CREATE TYPE AS ENUM requires labels".to_owned(),
            ));
        }
        labels.push(label.to_owned());
    }
    if labels.is_empty() {
        return Err(Error::UnsupportedSql(
            "CREATE TYPE AS ENUM requires labels".to_owned(),
        ));
    }
    Ok(labels)
}

fn domain_greater_than(rest: &str) -> Result<i64> {
    let lower = rest.to_ascii_lowercase();
    let Some(check_at) = lower.find("value") else {
        return Err(Error::UnsupportedSql(
            "CREATE DOMAIN requires CHECK (VALUE > n)".to_owned(),
        ));
    };
    let after = lower[check_at + "value".len()..].trim_start();
    let Some(after) = after.strip_prefix('>') else {
        return Err(Error::UnsupportedSql(
            "CREATE DOMAIN supports CHECK (VALUE > n)".to_owned(),
        ));
    };
    let number: String = after
        .trim_start()
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '-')
        .collect();
    number
        .parse::<i64>()
        .map_err(|_| Error::UnsupportedSql("CREATE DOMAIN supports CHECK (VALUE > n)".to_owned()))
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
        None
    } else {
        Some((&rest[..end], &rest[end..]))
    }
}

fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let mut chars = text.chars();
    for expected in prefix.chars() {
        let found = chars.next()?;
        if !found.eq_ignore_ascii_case(&expected) {
            return None;
        }
    }
    let rest = chars.as_str();
    if rest
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return None;
    }
    Some(rest)
}
