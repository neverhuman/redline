//! The compatibility rewrites leave every quoted form alone, in both
//! dialects: bracket and backtick names, comments, and Postgres `E'..'` and
//! `$tag$..$tag$` strings (Q5-01). End to end the Postgres dialect accepts
//! few of those string forms, so this checks the rewrite itself.

use super::rewrite_sqlite_compat_syntax;
use crate::connection::Dialect;
use crate::value::DialectScope;

/// Trigger phrases of the byte-wise passes.
const PHRASES: &[&str] = &[
    "a glob 'b*'",
    "a GLOB b",
    "a ? 'k'",
    ") strict, without rowid",
    "array[1,2]",
    "(a)[1]",
    "d - '-2 months'",
    "insert into t select 1 on conflict (a) do nothing",
    "select a into b from c",
    " group by grouping sets ((a),(b)) ",
];

fn unchanged(sql: &str) {
    assert_eq!(rewrite_sqlite_compat_syntax(sql), sql, "rewritten: `{sql}`");
}

#[test]
fn postgres_string_forms_are_not_rewritten() {
    let _scope = DialectScope::for_dialect(Dialect::PostgresSubset);
    for phrase in PHRASES {
        let sql = format!("SELECT $${phrase}$$"); // jankurai:allow HLT-023-INPUT-BOUNDARY-GAP reason=rewrite-test-fixture expires=2027-06-01
        unchanged(&sql);
        let sql = format!("SELECT $q${phrase}$q$"); // jankurai:allow HLT-023-INPUT-BOUNDARY-GAP reason=rewrite-test-fixture expires=2027-06-01
        unchanged(&sql);
        let sql = format!("SELECT E'{}'", phrase.replace('\'', "\\'")); // jankurai:allow HLT-023-INPUT-BOUNDARY-GAP reason=rewrite-test-fixture expires=2027-06-01
        unchanged(&sql);
        let sql = format!("SELECT $$'$$, 1 /* {phrase} */"); // jankurai:allow HLT-023-INPUT-BOUNDARY-GAP reason=rewrite-test-fixture expires=2027-06-01
        unchanged(&sql);
        let sql = format!("SELECT E'\\'', \"{phrase}\""); // jankurai:allow HLT-023-INPUT-BOUNDARY-GAP reason=rewrite-test-fixture expires=2027-06-01
        unchanged(&sql);
    }
}

#[test]
fn sqlite_quoted_names_and_comments_are_not_rewritten() {
    let _scope = DialectScope::for_dialect(Dialect::Sqlite);
    for phrase in PHRASES {
        if !phrase.contains(']') {
            let sql = format!("SELECT 7 AS [{phrase}]"); // jankurai:allow HLT-023-INPUT-BOUNDARY-GAP reason=rewrite-test-fixture expires=2027-06-01
            unchanged(&sql);
        }
        let sql = format!("SELECT 7 AS `{phrase}`"); // jankurai:allow HLT-023-INPUT-BOUNDARY-GAP reason=rewrite-test-fixture expires=2027-06-01
        unchanged(&sql);
        let sql = format!("SELECT 7 /* it's */, '{}'", phrase.replace('\'', "''")); // jankurai:allow HLT-023-INPUT-BOUNDARY-GAP reason=rewrite-test-fixture expires=2027-06-01
        unchanged(&sql);
        let sql = format!("SELECT 7 -- it's\n, '{}'", phrase.replace('\'', "''")); // jankurai:allow HLT-023-INPUT-BOUNDARY-GAP reason=rewrite-test-fixture expires=2027-06-01
        unchanged(&sql);
    }
}

#[test]
fn code_is_still_rewritten_around_quoted_forms() {
    let _scope = DialectScope::for_dialect(Dialect::Sqlite);
    let out = rewrite_sqlite_compat_syntax("SELECT [a GLOB b], x GLOB 'y*' /* it's */");
    assert!(out.starts_with("SELECT [a GLOB b], "), "{out}");
    assert!(out.to_ascii_lowercase().contains("glob('y*'"), "{out}");
    assert!(out.ends_with("/* it's */"), "{out}");
}
