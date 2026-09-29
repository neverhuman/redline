//! SQLite's wording for errors RedlineDB raises where sqlite3 raises the same
//! error. The sqlite_parity corpus holds each rejection to the text the
//! pinned sqlite3 3.53.1 prints, and `sqlite3_errmsg` in the C ABI returns
//! the same string, so the words live here rather than at each call site.
//!
//! The kernel reports a missing or duplicate catalog object without naming
//! it (`ObjectNotFound`, `ObjectExists`); the helpers below name it from the
//! statement and the committed schema after the kernel has refused.

use redlinedb_kernel::Error as KernelError;
use redlinedb_kernel::catalog::{
    DbName, IndexDef, IndexKeySource, IndexOrigin, QualifiedName, SchemaSnapshot, TableDef,
};
use sqlparser::ast::Expr;

use crate::connection::Connection;
use crate::error::Error;

pub(crate) const BEGIN_IN_TRANSACTION: &str = "cannot start a transaction within a transaction";
pub(crate) const COMMIT_WITHOUT_TRANSACTION: &str = "cannot commit - no transaction is active";
pub(crate) const ROLLBACK_WITHOUT_TRANSACTION: &str = "cannot rollback - no transaction is active";

/// `near "<token>": syntax error`, as sqlite3's parser reports it.
pub(crate) fn syntax_error_near(token: &str) -> Error {
    Error::Parse(format!("near \"{token}\": syntax error"))
}

/// A call no built-in or registered function answers. An aggregate lands
/// here only when it is nested where a scalar must run, which sqlite3
/// reports as a misuse of the aggregate.
pub(crate) fn unknown_function(name: &str, is_aggregate: bool) -> Error {
    if is_aggregate {
        Error::Sqlite(format!("misuse of aggregate function {name}()"))
    } else {
        Error::NoSuchFunction(name.to_owned())
    }
}

/// Name an unknown function as the statement spelled it (`ROLLUP`, not the
/// folded `rollup` the dispatcher matched on).
pub(crate) fn function_as_written(err: Error, written: &sqlparser::ast::ObjectName) -> Error {
    match err {
        Error::NoSuchFunction(_) => Error::NoSuchFunction(written.to_string()),
        other => other,
    }
}

/// `UNIQUE constraint failed: t.a, t.b`: the columns of the key that
/// collided, `t.rowid` for a bare rowid, or the index for an expression key.
pub(crate) fn unique_failed(
    table: &TableDef,
    key_ordinals: &[usize],
    expression_index: Option<&str>,
) -> Error {
    let target = match expression_index {
        Some(index) => format!("index '{index}'"),
        None if key_ordinals.is_empty() => format!("{}.rowid", table.name),
        None => key_ordinals
            .iter()
            .map(|&ordinal| match table.columns.get(ordinal) {
                Some(column) => format!("{}.{}", table.name, column.name),
                None => table.name.to_string(),
            })
            .collect::<Vec<_>>()
            .join(", "),
    };
    Error::ConstraintViolation(format!("UNIQUE constraint failed: {target}"))
}

/// `percentile_cont` called any way but `percentile_cont(value, fraction)`.
/// sqlite3's grammar has no `WITHIN GROUP`: the `(` after it is a syntax
/// error. Any other argument count is the wrong number of arguments.
pub(crate) fn percentile_cont_form(func: &sqlparser::ast::Function) -> Error {
    if crate::value::postgres_result_dialect() {
        Error::UnsupportedSql("percentile_cont requires two arguments".to_owned())
    } else if !func.within_group.is_empty() {
        syntax_error_near("(")
    } else {
        Error::Sqlite(format!(
            "wrong number of arguments to function {}()",
            func.name
        ))
    }
}

/// `RAISE()` evaluated outside a trigger program.
pub(crate) fn raise_outside_trigger() -> Error {
    Error::Sqlite("RAISE() may only be used within a trigger-program".to_owned())
}

/// A subquery used as a value whose result has the wrong number of columns.
pub(crate) fn sub_select_columns(returned: usize, expected: usize) -> Error {
    Error::Sqlite(format!(
        "sub-select returns {returned} columns - expected {expected}"
    ))
}

/// A `DELETE` clause sqlite3's grammar lacks (`ORDER BY`, `LIMIT`): its
/// syntax error in the SQLite dialect, `postgres` wording otherwise.
pub(crate) fn delete_clause(keyword: &str, postgres: &str) -> Error {
    if crate::value::postgres_result_dialect() {
        Error::UnsupportedSql(postgres.to_owned())
    } else {
        syntax_error_near(keyword)
    }
}

/// An expression the evaluator has no rule for. sqlite3's grammar has no
/// `= ANY (subquery)`, `> ALL (subquery)` or `GROUPING SETS`, so in the
/// SQLite dialect each is the syntax error sqlite3 reports, not a dump of
/// the parse tree.
pub(crate) fn unsupported_expression(expr: &Expr) -> Error {
    if !crate::value::postgres_result_dialect() {
        match expr {
            Expr::AnyOp { right, is_some, .. } => {
                return match right.as_ref() {
                    Expr::Subquery(query) => {
                        let text = query.to_string();
                        syntax_error_near(text.split_whitespace().next().unwrap_or("("))
                    }
                    _ => Error::NoSuchFunction(if *is_some { "SOME" } else { "ANY" }.to_owned()),
                };
            }
            Expr::AllOp { .. } => return syntax_error_near("ALL"),
            Expr::GroupingSets(_) => return syntax_error_near("SETS"),
            _ => {}
        }
    }
    Error::UnsupportedSql(format!("unsupported expression: {expr}"))
}

const SUBQUERY_IN_SCHEMA_EXPRESSION: &str = "subqueries prohibited in schema expressions";

/// A subquery inside an expression stored in the schema (a CHECK, a
/// generated column, an index expression), which no schema object may hold.
pub(crate) fn subquery_in_schema_expression() -> Error {
    Error::Sqlite(SUBQUERY_IN_SCHEMA_EXPRESSION.to_owned())
}

/// Name the clause in a subquery refusal from a CHECK constraint, as
/// sqlite3 does.
pub(crate) fn in_check_constraint(err: Error) -> Error {
    match err {
        Error::Sqlite(msg) if msg == SUBQUERY_IN_SCHEMA_EXPRESSION => {
            Error::Sqlite("subqueries prohibited in CHECK constraints".to_owned())
        }
        other => other,
    }
}

/// A statement the parser could not read. In the SQLite dialect the error
/// takes sqlite3's two shapes: `near "<token>": syntax error` at the token
/// the parser stopped on, and `incomplete input` when it ran out of SQL.
/// Tokenizer failures and the postgres dialect keep the parser's text.
pub(crate) fn parser_error(err: sqlparser::parser::ParserError) -> Error {
    use sqlparser::parser::ParserError;
    if crate::value::postgres_result_dialect() {
        return err.into();
    }
    let ParserError::ParserError(message) = &err else {
        return err.into();
    };
    let Some((_, found)) = message.rsplit_once("found: ") else {
        return err.into();
    };
    let token = found
        .rsplit_once(" at Line: ")
        .map_or(found, |(token, _)| token);
    if token == "EOF" {
        Error::Parse("incomplete input".to_owned())
    } else {
        syntax_error_near(token)
    }
}

/// The name sqlite3 gives a schema table in its errors, when `folded` is one.
fn schema_table_name(folded: &str) -> Option<&'static str> {
    match folded {
        "sqlite_master" | "sqlite_schema" => Some("sqlite_master"),
        "sqlite_temp_master" | "sqlite_temp_schema" => Some("sqlite_temp_master"),
        _ => None,
    }
}

fn not_found(err: &Error) -> bool {
    matches!(err, Error::Kernel(KernelError::ObjectNotFound))
}

fn has_table(schema: &SchemaSnapshot, name: &QualifiedName) -> bool {
    schema
        .lookup_namespace(name.schema.folded())
        .and_then(|id| schema.lookup_table(id, name.name.folded()))
        .is_some()
}

fn has_view(schema: &SchemaSnapshot, name: &QualifiedName) -> bool {
    schema
        .lookup_namespace(name.schema.folded())
        .and_then(|id| schema.lookup_view(id, name.name.folded()))
        .is_some()
}

/// A table named in DML or a query that does not exist. Writes to a schema
/// table fail the way sqlite3 refuses them.
pub(crate) fn missing_table(display: &str, folded: &str) -> Error {
    match schema_table_name(folded) {
        Some(table) => Error::Sqlite(format!("table {table} may not be modified")),
        None => Error::UnknownTable(display.to_owned()),
    }
}

/// `DROP TABLE` the kernel refused because no table has that name.
pub(crate) fn drop_table(conn: &Connection, name: &QualifiedName, err: Error) -> Error {
    if !not_found(&err) {
        return err;
    }
    let schema = &conn.schema_snapshot();
    let display = name.name.original();
    if let Some(table) = schema_table_name(name.name.folded()) {
        Error::Sqlite(format!("table {table} may not be dropped"))
    } else if has_view(schema, name) {
        Error::Sqlite(format!("use DROP VIEW to delete view {display}"))
    } else {
        Error::UnknownTable(display.to_owned())
    }
}

/// `DROP VIEW` the kernel refused because no view has that name.
pub(crate) fn drop_view(conn: &Connection, name: &QualifiedName, err: Error) -> Error {
    if !not_found(&err) {
        return err;
    }
    let schema = &conn.schema_snapshot();
    let display = name.name.original();
    if has_table(schema, name) {
        Error::Sqlite(format!("use DROP TABLE to delete table {display}"))
    } else {
        Error::Sqlite(format!("no such view: {display}"))
    }
}

/// `DROP INDEX` or `DROP TRIGGER` of an object that does not exist.
pub(crate) fn drop_object(kind: &str, name: &QualifiedName, err: Error) -> Error {
    if !not_found(&err) {
        return err;
    }
    Error::Sqlite(format!("no such {kind}: {}", name.name.original()))
}

/// `ALTER TABLE` the kernel refused because no table has that name.
pub(crate) fn alter_table(conn: &Connection, name: &QualifiedName, err: Error) -> Error {
    let schema = &conn.schema_snapshot();
    if !not_found(&err) || has_table(schema, name) {
        return err;
    }
    let display = name.name.original();
    if let Some(table) = schema_table_name(name.name.folded()) {
        Error::Sqlite(format!("table {table} may not be altered"))
    } else if has_view(schema, name) {
        Error::Sqlite(format!("view {display} may not be altered"))
    } else {
        Error::UnknownTable(display.to_owned())
    }
}

/// `ALTER TABLE t DROP COLUMN c` checked against `table` as sqlite3 checks
/// it before touching the schema: the column must exist, must not be the
/// last one, and no index the user created may use it. The kernel refuses
/// the other cases (a key or constrained column) in its own words.
pub(crate) fn check_drop_column(
    table: &TableDef,
    column: &str,
    if_exists: bool,
) -> Result<(), Error> {
    let folded = column.to_ascii_lowercase();
    let Some(dropped) = table.columns.iter().find(|c| *c.folded == folded) else {
        return if if_exists {
            Ok(())
        } else {
            Err(Error::Sqlite(format!("no such column: \"{column}\"")))
        };
    };
    if table.columns.len() == 1 {
        return Err(Error::Sqlite(format!(
            "cannot drop column \"{column}\": no other columns exist"
        )));
    }
    let uses_column = |index: &&IndexDef| {
        index.origin == IndexOrigin::User
            && index.keys.iter().any(|key| {
                matches!(key.source, IndexKeySource::Column { attnum } if attnum == dropped.ordinal)
            })
    };
    match table.indexes.iter().find(uses_column) {
        Some(index) => Err(Error::Sqlite(format!(
            "error in index {} after drop column: no such column: {column}",
            index.name
        ))),
        None => Ok(()),
    }
}

/// `CREATE TABLE` or `CREATE VIEW` the kernel refused because the name is
/// taken. sqlite3 names the kind of the object that already holds it.
pub(crate) fn create_object(
    conn: &Connection,
    namespace: Option<&DbName>,
    name: &DbName,
    err: Error,
) -> Error {
    if !matches!(err, Error::Kernel(KernelError::ObjectExists)) {
        return err;
    }
    let schema = &conn.schema_snapshot();
    let display = name.original();
    let namespace = namespace.map_or("main", DbName::folded);
    let Some(schema_id) = schema.lookup_namespace(namespace) else {
        return err;
    };
    let folded = name.folded();
    if schema.lookup_view(schema_id, folded).is_some() {
        Error::Sqlite(format!("view {display} already exists"))
    } else if schema.lookup_table(schema_id, folded).is_some() {
        Error::Sqlite(format!("table {display} already exists"))
    } else if schema.lookup_index(schema_id, folded).is_some() {
        Error::Sqlite(format!("there is already an index named {display}"))
    } else {
        err
    }
}
