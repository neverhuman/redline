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
        Ok(session.pg_enums.contains_key(&key) || session.pg_domains.contains_key(&key))
    })
    .unwrap_or(false)
}

/// `Some` when `type_name` is a registered enum or domain.
pub(crate) fn cast_registered(type_name: &str, value: &SqlValue) -> Result<Option<SqlValue>> {
    if !crate::value::postgres_result_dialect() {
        return Ok(None);
    }
    let Some(conn) = crate::exec::current_connection() else {
        return Ok(None);
    };
    let key = type_name.to_ascii_lowercase();
    let (labels, domain) = crate::exec::with_session_reentrant(&conn, |session| {
        Ok((
            session.pg_enums.get(&key).cloned(),
            session.pg_domains.get(&key).cloned(),
        ))
    })?;
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
