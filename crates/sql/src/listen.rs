//! Session `LISTEN` / `UNLISTEN` and the `pg_listening_channels()` read.
//!
//! Channels are connection-local. A transaction snapshots the set at
//! `BEGIN` and restores it on `ROLLBACK`, matching Postgres: a listen
//! that has not committed is gone after rollback.

use std::ops::Range;
use std::sync::Arc;

use sqlparser::ast::Ident;

use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::parser::code_scan::{Lexer, is_word_byte, skip_trivia, sql_code_mask};
use crate::parser::find_ignore_ascii_case;
use crate::session::SessionState;
use crate::statement::PreparedTemplate;
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

const LISTENING_CHANNELS: &str = "pg_listening_channels";

/// Keywords that may follow the select list of a FROM-less SELECT, where the
/// rewrite can insert its `FROM` source.
const SELECT_LIST_FOLLOWERS: &[&str] = &[
    "order",
    "limit",
    "offset",
    "fetch",
    "where",
    "group",
    "having",
    "window",
    "union",
    "except",
    "intersect",
];

/// Rewrite `SELECT pg_listening_channels()` into a one-column `FROM` source
/// holding the session's current channels. Set-returning functions are not
/// executed, so only the call as the whole select list of a SELECT is
/// supported (the shell corpus uses it alone and in `count(*)` subqueries).
/// Only code is rewritten: the same text in a literal, quoted identifier or
/// comment is left alone (PG-02), and any other use of the call is refused
/// instead of being rewritten into a different statement.
pub(crate) fn rewrite_pg_listening_channels(
    conn: &Connection,
    sql: &str,
) -> Result<Option<String>> {
    if !postgres_result_dialect() || !sql_reads_listening_channels(sql) {
        return Ok(None);
    }
    let calls = listening_calls(sql)?;
    if calls.is_empty() {
        return Ok(None);
    }
    let channels = crate::exec::with_session_reentrant(conn, |session| {
        Ok(session.pg_listening.iter().cloned().collect::<Vec<_>>())
    })?;
    let source = listening_from(&channels);
    let mut out = String::with_capacity(sql.len() + source.len());
    let mut copied = 0usize;
    for call in calls {
        out.push_str(&sql[copied..call.start]);
        out.push_str("pg_listening_channels FROM ");
        out.push_str(&source);
        copied = call.end;
    }
    out.push_str(&sql[copied..]);
    Ok(Some(out))
}

/// Byte spans of the `pg_listening_channels()` calls in the code of `sql`.
fn listening_calls(sql: &str) -> Result<Vec<Range<usize>>> {
    let mask = code_mask(sql);
    let bytes = sql.as_bytes();
    let mut calls = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = find_ignore_ascii_case(&sql[from..], LISTENING_CHANNELS.as_bytes()) {
        let at = from + rel;
        let after = at + LISTENING_CHANNELS.len();
        from = after;
        let whole_word = (at == 0 || !is_word_byte(bytes[at - 1]))
            && !bytes.get(after).is_some_and(|&b| is_word_byte(b));
        if !mask[at] || !whole_word {
            continue;
        }
        let open = skip_trivia(bytes, after, Lexer::Postgres);
        if bytes.get(open) != Some(&b'(') {
            // A column or alias named `pg_listening_channels`, such as the
            // one this rewrite emits.
            continue;
        }
        let close = skip_trivia(bytes, open + 1, Lexer::Postgres);
        if bytes.get(close) != Some(&b')')
            || !select_keyword_before(sql, &mask, at)
            || !select_list_ends_at(sql, close + 1)
        {
            return Err(Error::UnsupportedCapability {
                feature: "pg_listening_channels outside a bare SELECT list",
                detail: "only `SELECT pg_listening_channels()`, alone in its select list, \
                         is supported"
                    .to_owned(),
            });
        }
        calls.push(at..close + 1);
        from = close + 1;
    }
    Ok(calls)
}

/// True when the code just before `at`, past whitespace, is the keyword
/// SELECT.
fn select_keyword_before(sql: &str, mask: &[bool], at: usize) -> bool {
    let bytes = sql.as_bytes();
    let mut end = at;
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    let Some(start) = end.checked_sub("select".len()) else {
        return false;
    };
    sql.get(start..end)
        .is_some_and(|word| word.eq_ignore_ascii_case("select"))
        && mask[start]
        && (start == 0 || !is_word_byte(bytes[start - 1]))
}

/// True when the select list may end at `pos`: the input, a `;` or `)`
/// ends there, or a clause keyword that can follow a select list starts.
fn select_list_ends_at(sql: &str, pos: usize) -> bool {
    let bytes = sql.as_bytes();
    let next = skip_trivia(bytes, pos, Lexer::Postgres);
    match bytes.get(next) {
        None | Some(b';') | Some(b')') => true,
        Some(_) => {
            let mut end = next;
            while end < bytes.len() && is_word_byte(bytes[end]) {
                end += 1;
            }
            let word = &sql[next..end];
            SELECT_LIST_FOLLOWERS
                .iter()
                .any(|keyword| word.eq_ignore_ascii_case(keyword))
        }
    }
}

/// One flag per byte of `sql`: `true` where the byte is code, `false`
/// inside `'..'` (with `''`), `E'..'` (with backslash escapes), `".."`,
/// `$tag$..$tag$`, `--` comments and nested `/* */` comments.
fn code_mask(sql: &str) -> Vec<bool> {
    sql_code_mask(sql, Lexer::Postgres)
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

/// The select-list rewrite embeds the current channel list in the prepared
/// template. `LISTEN` does not change `schema_epoch`, so that template must
/// not be reused from the statement cache, and a held statement must be
/// prepared again before each execution ([`template_reads_channels`]). This
/// over-approximates: text in a literal also counts.
pub(crate) fn sql_reads_listening_channels(sql: &str) -> bool {
    find_ignore_ascii_case(sql, LISTENING_CHANNELS.as_bytes()).is_some()
}

/// True when `template` embeds the channel set, so a statement must prepare
/// it again before it runs: `LISTEN` may have changed the set since. The
/// rewrite keeps the SQL it started from on the template for that purpose.
pub(crate) fn template_reads_channels(template: &PreparedTemplate) -> bool {
    postgres_result_dialect() && sql_reads_listening_channels(&template.sql)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(sql: &str) -> String {
        let mask = code_mask(sql);
        sql.char_indices()
            .filter(|(at, _)| mask[*at])
            .map(|(_, ch)| ch)
            .collect()
    }

    #[test]
    fn code_mask_skips_every_quoting_form() {
        assert_eq!(code("SELECT 'a''b' x"), "SELECT  x");
        assert_eq!(code("SELECT E'a\\'b' x"), "SELECT  x");
        assert_eq!(code("SELECT \"a\"\"b\" x"), "SELECT  x");
        assert_eq!(code("SELECT $$a'b$$ x"), "SELECT  x");
        assert_eq!(code("SELECT $t$a$$b$t$ x"), "SELECT  x");
        assert_eq!(code("SELECT 1 -- a\nx"), "SELECT 1 \nx");
        assert_eq!(code("SELECT /* a /* b */ c */ x"), "SELECT  x");
        assert_eq!(code("SELECT 'é' x"), "SELECT  x");
    }

    #[test]
    fn code_mask_keeps_parameters_and_words() {
        assert_eq!(code("SELECT $1, a$b, e'x'"), "SELECT $1, a$b, ");
        assert_eq!(code("SELECT sizE'a' x"), "SELECT sizE x");
    }

    #[test]
    fn calls_in_literals_and_comments_are_not_calls() {
        for sql in [
            "SELECT 'SELECT pg_listening_channels()'",
            "SELECT E'\\' SELECT pg_listening_channels()'",
            "SELECT $$SELECT pg_listening_channels()$$",
            "SELECT $q$SELECT pg_listening_channels()$q$",
            "SELECT 1 AS \"SELECT pg_listening_channels()\"",
            "SELECT 1 -- SELECT pg_listening_channels()",
            "SELECT 1 /* /* */ SELECT pg_listening_channels() */",
            "SELECT pg_listening_channels FROM t",
        ] {
            assert!(listening_calls(sql).expect(sql).is_empty(), "{sql}");
        }
    }

    #[test]
    fn supported_and_unsupported_calls() {
        assert_eq!(
            listening_calls("SELECT pg_listening_channels()").unwrap(),
            vec![7..30]
        );
        assert_eq!(
            listening_calls("/* é */ Select PG_LISTENING_CHANNELS ( ) ORDER BY 1").unwrap(),
            vec![16..41]
        );
        for sql in [
            "SELECT pg_listening_channels(), 1",
            "SELECT 1, pg_listening_channels()",
            "SELECT upper(pg_listening_channels())",
            "SELECT * FROM pg_listening_channels()",
            "SELECT pg_listening_channels() AS c",
        ] {
            assert!(listening_calls(sql).is_err(), "{sql}");
        }
    }
}
