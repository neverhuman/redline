#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use redlinedb_sql::{split_statements, Database, DbOptions, Dialect, SqlValue, Step};
use rusqlite::types::Value as RuValue;

#[path = "../src/oracle.rs"]
mod oracle;

const MAX_STATEMENTS: usize = 16;
const MAX_ROWS: usize = 64;
const MAX_COLUMNS: usize = 32;
const MAX_STEPS: usize = 128;

enum Outcome {
    Err(String),
    Executed,
    Rows(Vec<Vec<SqlValue>>),
    Truncated,
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() || data.len() > 4 * 1024 {
        return;
    }
    let Ok(sql) = std::str::from_utf8(data) else {
        return;
    };
    if oracle::skips_filesystem(sql)
        || oracle::skips_recursive(sql)
        || oracle::numbered_parameter_is_unbounded(sql)
    {
        return;
    }
    let statements = split_statements(sql);
    if statements.is_empty() || statements.len() > MAX_STATEMENTS {
        return;
    }
    oracle::note_input();
    let sqlite = match rusqlite::Connection::open_in_memory() {
        Ok(connection) => connection,
        Err(err) => panic!("sqlite in-memory open failed: {err}"),
    };
    let redline = redline_connection();
    for statement in statements {
        let expected = sqlite_one(&sqlite, statement);
        let actual = redline_one(&redline, statement);
        compare(statement, expected, actual);
    }
});

fn redline_connection() -> Arc<redlinedb_sql::Connection> {
    let mut opts = DbOptions::default();
    opts.dialect = Some(Dialect::Sqlite);
    if let Some(dir) = std::env::var_os("REDLINE_FUZZ_TMP") {
        opts.temp_dir = Some(dir.into());
    }
    let db = Database::create_in_memory(opts).expect("redline in-memory database");
    db.connect()
}

fn from_sqlite(value: RuValue) -> SqlValue {
    match value {
        RuValue::Null => SqlValue::Null,
        RuValue::Integer(value) => SqlValue::Integer(value),
        RuValue::Real(value) => SqlValue::Real(value),
        RuValue::Text(value) => SqlValue::Text(value.into()),
        RuValue::Blob(value) => SqlValue::Blob(value.into()),
    }
}

fn sqlite_one(connection: &rusqlite::Connection, sql: &str) -> Outcome {
    let mut statement = match connection.prepare(sql) {
        Ok(statement) => statement,
        Err(err) => return Outcome::Err(err.to_string()),
    };
    if statement.column_count() == 0 {
        return match statement.execute([]) {
            Ok(_) => Outcome::Executed,
            Err(err) => Outcome::Err(err.to_string()),
        };
    }
    let width = statement.column_count().min(MAX_COLUMNS);
    let mut rows = Vec::new();
    let mut cursor = statement.raw_query();
    let mut steps = 0usize;
    loop {
        steps += 1;
        if steps > MAX_STEPS || rows.len() >= MAX_ROWS {
            return Outcome::Truncated;
        }
        match cursor.next() {
            Ok(Some(row)) => {
                let mut values = Vec::with_capacity(width);
                for index in 0..width {
                    match row.get(index) {
                        Ok(value) => values.push(from_sqlite(value)),
                        Err(err) => return Outcome::Err(err.to_string()),
                    }
                }
                rows.push(values);
            }
            Ok(None) => break,
            Err(err) => return Outcome::Err(err.to_string()),
        }
    }
    Outcome::Rows(rows)
}

fn redline_one(connection: &Arc<redlinedb_sql::Connection>, sql: &str) -> Outcome {
    let mut statement = match connection.prepare(sql) {
        Ok(statement) => statement,
        Err(err) => {
            if oracle::parser_panic(&err) {
                panic!("{err}");
            }
            return Outcome::Err(err.to_string());
        }
    };
    if statement.column_count() == 0 {
        loop {
            match statement.step() {
                Ok(Step::Row) | Ok(Step::Done) => break,
                Err(err) => {
                    if oracle::parser_panic(&err) {
                        panic!("{err}");
                    }
                    return Outcome::Err(err.to_string());
                }
            }
        }
        return Outcome::Executed;
    }
    let width = statement.column_count().min(MAX_COLUMNS);
    let mut rows = Vec::new();
    let mut steps = 0usize;
    loop {
        steps += 1;
        if steps > MAX_STEPS || rows.len() >= MAX_ROWS {
            return Outcome::Truncated;
        }
        match statement.step() {
            Ok(Step::Row) => {
                let mut values = Vec::with_capacity(width);
                for index in 0..width {
                    match statement.column_value(index) {
                        Ok(value) => values.push(value.clone()),
                        Err(err) => return Outcome::Err(err.to_string()),
                    }
                }
                rows.push(values);
            }
            Ok(Step::Done) => break,
            Err(err) => {
                if oracle::parser_panic(&err) {
                    panic!("{err}");
                }
                return Outcome::Err(err.to_string());
            }
        }
    }
    Outcome::Rows(rows)
}

fn compare(sql: &str, expected: Outcome, actual: Outcome) {
    match (expected, actual) {
        (Outcome::Err(expected), Outcome::Err(actual)) => {
            let expected_class = oracle::error_class(&expected);
            let actual_class = oracle::error_class(&actual);
            if expected_class != actual_class {
                oracle::note_divergence(
                    "error-class",
                    sql,
                    &format!("sqlite={expected_class} redline={actual_class}"),
                );
            }
        }
        (Outcome::Executed, Outcome::Executed) => {}
        (Outcome::Rows(expected), Outcome::Rows(mut actual)) => {
            let mut expected = expected;
            if oracle::same_rows(&expected, &actual) {
                return;
            }
            oracle::sort_rows(&mut expected);
            oracle::sort_rows(&mut actual);
            if oracle::same_rows(&expected, &actual) {
                oracle::note_divergence("row-order", sql, "sorted rows match");
            } else {
                oracle::note_divergence(
                    "row-values",
                    sql,
                    &format!(
                        "sqlite_rows={} redline_rows={}",
                        expected.len(),
                        actual.len()
                    ),
                );
            }
        }
        (Outcome::Err(expected), other) | (other, Outcome::Err(expected)) => {
            let side = match other {
                Outcome::Err(_) => "redline-error",
                Outcome::Executed => "executed",
                Outcome::Truncated => "truncated",
                Outcome::Rows(rows) => {
                    return oracle::note_divergence(
                        "success-vs-error",
                        sql,
                        &format!(
                            "class={} rows={}",
                            oracle::error_class(&expected),
                            rows.len()
                        ),
                    );
                }
            };
            oracle::note_divergence(
                "success-vs-error",
                sql,
                &format!("class={} other={side}", oracle::error_class(&expected)),
            );
        }
        (Outcome::Executed, Outcome::Rows(rows)) | (Outcome::Rows(rows), Outcome::Executed) => {
            oracle::note_divergence(
                "shape",
                sql,
                &format!(
                    "one side returned {} rows and the other executed",
                    rows.len()
                ),
            );
        }
        (Outcome::Truncated, Outcome::Truncated) => {}
        (Outcome::Truncated, _) | (_, Outcome::Truncated) => {
            oracle::note_divergence(
                "truncated",
                sql,
                "one side hit the step or row cap and the other finished",
            );
        }
    }
}
