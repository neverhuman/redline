//! Binding again at execution a statement whose binding read rows (Q5-08).
//!
//! Views, CTEs and derived tables are materialized while a statement is
//! bound. A statement whose binding did that is bound again each time it is
//! executed, with the parameters bound for that execution, so it reads the
//! rows of the moment it runs. The one exception is the first execution
//! straight after the preparation: when nothing is bound and nothing has
//! committed or been written by this connection since, the rows the
//! preparation read are still the rows of the moment.

use std::cell::RefCell;

use crate::connection::Connection;
use crate::error::Result;
use crate::statement::ParamLayout;
use crate::value::SqlValue;

thread_local! {
    /// The parameters of the statement being bound again, innermost last.
    static BIND_TIME_BINDINGS: RefCell<Vec<Vec<Option<SqlValue>>>> =
        const { RefCell::new(Vec::new()) };
}

/// What the rows a binding read depend on besides the schema: every commit
/// in the database, and this connection's transaction and its writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DataVersion {
    published_csn: u64,
    transaction: u64,
    total_changes: usize,
}

impl DataVersion {
    pub(crate) fn of(conn: &Connection) -> Result<Self> {
        Self::at(conn, Self::published_csn(conn))
    }

    /// The last commit visible to a snapshot taken now. A preparation
    /// reads it before it binds, so a commit that lands while it binds
    /// counts as a change.
    pub(crate) fn published_csn(conn: &Connection) -> u64 {
        conn.engine().tx_status().published_csn().0
    }

    /// The version with `published_csn` read earlier, and this connection's
    /// transaction and writes as they are now.
    pub(crate) fn at(conn: &Connection, published_csn: u64) -> Result<Self> {
        let (transaction, total_changes) = super::with_session_reentrant(conn, |session| {
            Ok((
                session.tx.as_ref().map_or(0, |tx| tx.id().0),
                session.total_changes,
            ))
        })?;
        Ok(Self {
            published_csn,
            transaction,
            total_changes,
        })
    }
}

/// Makes a statement's parameters visible to the materializers while it is
/// bound again.
pub(crate) struct BindTimeBindings;

impl BindTimeBindings {
    pub(crate) fn install(bindings: &[Option<SqlValue>]) -> Self {
        BIND_TIME_BINDINGS.with(|stack| stack.borrow_mut().push(bindings.to_vec()));
        Self
    }
}

impl Drop for BindTimeBindings {
    fn drop(&mut self) {
        BIND_TIME_BINDINGS.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}

/// The parameters a bind-time materialization evaluates with: the running
/// statement's while it is bound again, none during a first preparation.
pub(crate) fn bind_time_bindings() -> Vec<Option<SqlValue>> {
    BIND_TIME_BINDINGS.with(|stack| stack.borrow().last().cloned().unwrap_or_default())
}

/// The text to bind for a statement's `sql`, and the parameter layout of
/// the whole statement, when the text has a `?` or a named parameter:
/// every parameter written `?N` in SQLite's numbering. A subquery, CTE or
/// derived table is bound in a pass of its own; with the numbering spelled
/// out it binds `?N` to the statement's slot N instead of restarting at 1
/// (`SELECT ?, (SELECT ?)` read the first value twice). `None` in the
/// Postgres dialect, and for text the SQLite grammar does not accept as
/// written: there `?` can be the JSON `?`, `?|` or `?&` operator, which
/// the Postgres grammar parses.
pub(crate) fn canonical_for_prepare(sql: &str) -> Option<(String, ParamLayout)> {
    if crate::value::postgres_result_dialect()
        || !sql
            .bytes()
            .any(|byte| matches!(byte, b'?' | b':' | b'@' | b'$'))
    {
        return None;
    }
    let mut layout = ParamLayout::default();
    crate::parser::scan_sql_parameters(sql, &mut layout);
    if layout.count() == 0 {
        return None;
    }
    let canonical = canonical_placeholders(sql, &layout)?;
    if canonical != sql
        && sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::SQLiteDialect {}, sql).is_err()
    {
        return None;
    }
    Some((canonical, layout))
}

/// `sql` with every parameter written as `?N`, `N` being the slot `layout`
/// gives it, so a subquery bound on its own numbers its parameters as the
/// whole statement does (the text is unchanged when every parameter is
/// already `?N`). `None` when the text holds a construct the rewrite does
/// not handle (a `::` cast, a `$`-quoted body) or `layout` disagrees with
/// what the rewrite found: the caller binds the text as written.
pub(crate) fn canonical_placeholders(sql: &str, layout: &ParamLayout) -> Option<String> {
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len() + 8);
    let mut seen = ParamLayout::default();
    let mut i = 0usize;
    let mut copied = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"' | b'`') => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == quote {
                        if bytes.get(i + 1) == Some(&quote) {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
            }
            b'[' => {
                while i < bytes.len() && bytes[i] != b']' {
                    i += 1;
                }
                i += 1;
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < bytes.len() && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i += 2;
            }
            b':' if bytes.get(i + 1) == Some(&b':') => return None,
            b'?' => {
                let start = i;
                i += 1;
                let digits = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i > digits {
                    let index = sql[digits..i].parse::<usize>().ok()?;
                    seen.push_numbered(index);
                } else {
                    let slot = seen.push_anonymous();
                    out.push_str(&sql[copied..start]);
                    out.push_str(&format!("?{slot}"));
                    copied = i;
                }
            }
            b':' | b'@' | b'$' => {
                let start = i;
                i += 1;
                while i < bytes.len() && crate::parser::is_param_char(bytes[i]) {
                    i += 1;
                }
                if i == start + 1 {
                    if bytes[start] == b'$' {
                        return None;
                    }
                    continue;
                }
                if bytes[start] == b'$' && bytes.get(i) == Some(&b'$') {
                    return None;
                }
                let slot = seen.push_named(sql[start..i].to_owned());
                out.push_str(&sql[copied..start]);
                out.push_str(&format!("?{slot}"));
                copied = i;
            }
            _ => i += 1,
        }
    }
    if seen.count() != layout.count() {
        return None;
    }
    out.push_str(&sql[copied.min(sql.len())..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout_of(sql: &str) -> ParamLayout {
        let mut layout = ParamLayout::default();
        crate::parser::scan_sql_parameters(sql, &mut layout);
        layout
    }

    fn canonical(sql: &str) -> Option<String> {
        canonical_placeholders(sql, &layout_of(sql))
    }

    #[test]
    fn placeholders_take_the_numbers_of_the_whole_statement() {
        assert_eq!(
            canonical("SELECT ?, (SELECT a FROM (SELECT ? AS a)) WHERE ? > 0").as_deref(),
            Some("SELECT ?1, (SELECT a FROM (SELECT ?2 AS a)) WHERE ?3 > 0")
        );
        assert_eq!(
            canonical("SELECT :a, (SELECT :b), :a, ?").as_deref(),
            Some("SELECT ?1, (SELECT ?2), ?1, ?3")
        );
        assert_eq!(
            canonical("SELECT ?2, ?, @x").as_deref(),
            Some("SELECT ?2, ?3, ?4")
        );
        // Quoted text, identifiers and comments are left alone.
        let mut one = ParamLayout::default();
        one.push_anonymous();
        assert_eq!(
            canonical_placeholders("SELECT '?', \"?\", [?], `:x` -- ?\n, ? /* :y */", &one)
                .as_deref(),
            Some("SELECT '?', \"?\", [?], `:x` -- ?\n, ?1 /* :y */")
        );
        // The layout disagrees with what the rewrite found: leave the text.
        assert_eq!(canonical("SELECT [?], ?"), None);
        // A `?` the SQLite grammar does not take as a parameter.
        assert!(canonical_for_prepare("SELECT '{\"a\":1}' ? 'a'").is_none());
        assert!(canonical_for_prepare("SELECT '{}' ?| ARRAY['a']").is_none());
        assert!(canonical_for_prepare("SELECT ?, (SELECT ?)").is_some());
        // Nothing to rewrite, or a construct the rewrite does not handle.
        assert_eq!(canonical("SELECT ?1, ?2").as_deref(), Some("SELECT ?1, ?2"));
        assert_eq!(canonical("SELECT 1").as_deref(), Some("SELECT 1"));
        assert_eq!(canonical("SELECT ?::text"), None);
        assert_eq!(canonical("SELECT $$?$$"), None);
    }
}
