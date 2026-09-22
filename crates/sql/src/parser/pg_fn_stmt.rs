//! `CREATE FUNCTION ... LANGUAGE SQL` and `DROP FUNCTION` for the shell corpus.
//!
//! The SQLite dialect does not accept these statements. The shapes below are
//! the ones the Postgres cases use: a name, typed arguments, an optional
//! default, volatility and security words, and a dollar-quoted `SELECT`.

use std::sync::Arc;

use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::statement::{PreparedKind, PreparedTemplate};

use super::templates::template;

pub(crate) fn try_prepare(conn: &Connection, sql: &str) -> Result<Option<PreparedTemplate>> {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    let schema_epoch = conn.schema_epoch();
    if let Some(rest) = strip_prefix_ci(trimmed, "drop function") {
        let (name, if_exists) = parse_drop(rest)?;
        return Ok(Some(template(
            sql,
            schema_epoch,
            false,
            PreparedKind::DropSqlFn {
                name: Arc::from(name),
                if_exists,
            },
        )));
    }
    if let Some(rest) = strip_prefix_ci(trimmed, "create function") {
        let spec = parse_create(rest)?;
        return Ok(Some(template(
            sql,
            schema_epoch,
            false,
            PreparedKind::CreateSqlFn {
                name: Arc::from(spec.name),
                arg_names: spec.arg_names,
                defaults: spec.defaults,
                body: Arc::from(spec.body),
                security_definer: spec.security_definer,
            },
        )));
    }
    Ok(None)
}

struct CreateSpec {
    name: String,
    arg_names: Vec<String>,
    defaults: Vec<Option<String>>,
    body: String,
    security_definer: bool,
}

fn parse_drop(rest: &str) -> Result<(String, bool)> {
    let if_exists = strip_prefix_ci(rest.trim_start(), "if exists").is_some();
    let rest = if if_exists {
        strip_prefix_ci(rest.trim_start(), "if exists").unwrap_or(rest)
    } else {
        rest
    };
    let (name, _) = take_ident(rest.trim_start())
        .ok_or_else(|| Error::UnsupportedSql("DROP FUNCTION requires a name".to_owned()))?;
    Ok((name.to_ascii_lowercase(), if_exists))
}

fn parse_create(rest: &str) -> Result<CreateSpec> {
    let (name, rest) = take_ident(rest.trim_start())
        .ok_or_else(|| Error::UnsupportedSql("CREATE FUNCTION requires a name".to_owned()))?;
    let rest = rest.trim_start();
    if !rest.starts_with('(') {
        return Err(Error::UnsupportedSql(
            "CREATE FUNCTION requires an argument list".to_owned(),
        ));
    }
    let (args_src, rest) = take_paren(&rest[1..])?;
    let (arg_names, defaults) = parse_args(args_src)?;
    let mut rest = rest.trim_start();
    if let Some(after) = strip_prefix_ci(rest, "returns") {
        let (_ty, after_ty) = take_ident(after.trim_start()).ok_or_else(|| {
            Error::UnsupportedSql("CREATE FUNCTION requires a return type".to_owned())
        })?;
        rest = after_ty.trim_start();
    }
    let Some(after_lang) = strip_prefix_ci(rest, "language") else {
        return Err(Error::UnsupportedSql(
            "CREATE FUNCTION requires LANGUAGE SQL".to_owned(),
        ));
    };
    let (language, after_lang) = take_ident(after_lang.trim_start())
        .ok_or_else(|| Error::UnsupportedSql("CREATE FUNCTION requires LANGUAGE SQL".to_owned()))?;
    if !language.eq_ignore_ascii_case("sql") {
        return Err(Error::UnsupportedSql(
            "only LANGUAGE SQL functions are supported".to_owned(),
        ));
    }
    let mut rest = after_lang.trim_start();
    if strip_prefix_ci(rest, "immutable").is_some() {
        rest = strip_prefix_ci(rest, "immutable")
            .unwrap_or(rest)
            .trim_start();
    } else if strip_prefix_ci(rest, "stable").is_some() {
        rest = strip_prefix_ci(rest, "stable").unwrap_or(rest).trim_start();
    } else if strip_prefix_ci(rest, "volatile").is_some() {
        rest = strip_prefix_ci(rest, "volatile")
            .unwrap_or(rest)
            .trim_start();
    }
    let mut security_definer = false;
    if let Some(after) = strip_prefix_ci(rest, "security") {
        let (which, after_which) = take_ident(after.trim_start()).ok_or_else(|| {
            Error::UnsupportedSql("SECURITY requires DEFINER or INVOKER".to_owned())
        })?;
        security_definer = which.eq_ignore_ascii_case("definer");
        if !security_definer && !which.eq_ignore_ascii_case("invoker") {
            return Err(Error::UnsupportedSql(
                "SECURITY requires DEFINER or INVOKER".to_owned(),
            ));
        }
        rest = after_which.trim_start();
    }
    let Some(after_as) = strip_prefix_ci(rest, "as") else {
        return Err(Error::UnsupportedSql(
            "CREATE FUNCTION requires AS".to_owned(),
        ));
    };
    let (body, _after) = take_dollar_body(after_as.trim_start())?;
    let body = body.trim().trim_end_matches(';').trim().to_owned();
    if body.is_empty() {
        return Err(Error::UnsupportedSql(
            "CREATE FUNCTION requires a body".to_owned(),
        ));
    }
    Ok(CreateSpec {
        name: name.to_ascii_lowercase(),
        arg_names,
        defaults,
        body,
        security_definer,
    })
}

fn parse_args(src: &str) -> Result<(Vec<String>, Vec<Option<String>>)> {
    if src.trim().is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let mut names = Vec::new();
    let mut defaults = Vec::new();
    for piece in split_args(src) {
        let (name, rest) = take_ident(piece.trim())
            .ok_or_else(|| Error::UnsupportedSql("function argument requires a name".to_owned()))?;
        let rest = rest.trim_start();
        let (default, _) = split_default(rest);
        names.push(name.to_ascii_lowercase());
        defaults.push(default);
    }
    Ok((names, defaults))
}

fn split_default(rest: &str) -> (Option<String>, ()) {
    let lower = rest.to_ascii_lowercase();
    let Some(at) = lower.find(" default ") else {
        return (None, ());
    };
    let literal = rest[at + " default ".len()..].trim();
    if literal.is_empty() {
        (None, ())
    } else {
        (Some(literal.to_owned()), ())
    }
}

fn split_args(src: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    let mut in_str = false;
    let bytes = src.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if in_str {
            if b == b'\'' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    i += 2;
                    continue;
                }
                in_str = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'\'' => in_str = true,
            b'(' => depth += 1,
            b')' => depth -= 1,
            b',' if depth == 0 => {
                out.push(&src[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(&src[start..]);
    out
}

fn take_paren(src: &str) -> Result<(&str, &str)> {
    let bytes = src.as_bytes();
    let mut depth = 1i32;
    let mut in_str = false;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if in_str {
            if b == b'\'' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    i += 2;
                    continue;
                }
                in_str = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'\'' => in_str = true,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok((&src[..i], &src[i + 1..]));
                }
            }
            _ => {}
        }
        i += 1;
    }
    Err(Error::UnsupportedSql(
        "CREATE FUNCTION argument list is not closed".to_owned(),
    ))
}

fn take_dollar_body(src: &str) -> Result<(&str, &str)> {
    if !src.starts_with('$') {
        return Err(Error::UnsupportedSql(
            "CREATE FUNCTION body must be dollar-quoted".to_owned(),
        ));
    }
    let bytes = src.as_bytes();
    let mut i = 1usize;
    while i < bytes.len() && bytes[i] != b'$' {
        i += 1;
    }
    if i >= bytes.len() {
        return Err(Error::UnsupportedSql(
            "CREATE FUNCTION body must be dollar-quoted".to_owned(),
        ));
    }
    let tag = &src[..=i];
    let rest = &src[tag.len()..];
    let Some(close) = rest.find(tag) else {
        return Err(Error::UnsupportedSql(
            "CREATE FUNCTION body is not closed".to_owned(),
        ));
    };
    Ok((&rest[..close], &rest[close + tag.len()..]))
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
    let prefix_len = prefix.len();
    if prefix_len > sql.len() || !sql.is_char_boundary(prefix_len) {
        return None;
    }
    if !sql[..prefix_len].eq_ignore_ascii_case(prefix) {
        return None;
    }
    let rest = &sql[prefix_len..];
    if rest.is_empty() || rest.as_bytes()[0].is_ascii_whitespace() {
        Some(rest)
    } else {
        None
    }
}
