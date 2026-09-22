//! Postgres requires the `DISTINCT ON` expressions to be the leftmost
//! `ORDER BY` terms and rejects the query otherwise. Before this gate
//! RedlineDB accepted the mismatched form and returned a row, which is a
//! divergence in the dangerous direction: accepting a query the reference
//! rejects, and then picking the surviving row arbitrarily.
//!
//! Reference behaviour, PostgreSQL 16.15:
//!
//! ```text
//! ERROR:  SELECT DISTINCT ON expressions must match initial ORDER BY expressions
//! ```

use redlinedb_sql::{Connection, Database, DbOptions, Step};
use std::sync::Arc;
use tempfile::tempdir;

fn open() -> (tempfile::TempDir, Arc<Connection>) {
    let dir = tempdir().expect("temp dir");
    let db =
        Database::create(&dir.path().join("distinct_on.db"), DbOptions::default()).expect("db");
    let conn = db.connect();
    conn.execute("CREATE TABLE d (g int, h int, x text)")
        .expect("create");
    conn.execute("INSERT INTO d VALUES (1,2,'a'),(1,2,'b'),(2,3,'c')")
        .expect("insert");
    (dir, conn)
}

fn row_count(conn: &Arc<Connection>, sql: &str) -> usize {
    let mut stmt = conn.prepare(sql).expect("prepare");
    let mut n = 0;
    while let Step::Row = stmt.step().expect("step") {
        n += 1;
    }
    n
}

fn rejection(conn: &Arc<Connection>, sql: &str) -> String {
    match conn.prepare(sql) {
        Ok(_) => panic!("expected `{sql}` to be rejected"),
        Err(err) => err.to_string(),
    }
}

#[test]
fn order_by_that_does_not_lead_with_the_on_expression_is_rejected() {
    let (_d, c) = open();
    let message = rejection(&c, "SELECT DISTINCT ON (g) g, x FROM d ORDER BY x");
    assert!(
        message.contains("SELECT DISTINCT ON expressions must match initial ORDER BY expressions"),
        "unexpected message: {message}"
    );
}

#[test]
fn order_by_shorter_than_the_on_list_is_rejected() {
    let (_d, c) = open();
    let message = rejection(&c, "SELECT DISTINCT ON (g, h) g, h, x FROM d ORDER BY g");
    assert!(
        message.contains("SELECT DISTINCT ON expressions must match initial ORDER BY expressions"),
        "unexpected message: {message}"
    );
}

#[test]
fn order_by_leading_with_the_on_expression_is_accepted() {
    let (_d, c) = open();
    assert_eq!(
        row_count(&c, "SELECT DISTINCT ON (g) g, x FROM d ORDER BY g, x"),
        2
    );
}

#[test]
fn trailing_order_by_terms_break_ties_and_are_allowed() {
    let (_d, c) = open();
    // `h` is neither in the ON list nor leading; it only orders within a group.
    assert_eq!(
        row_count(&c, "SELECT DISTINCT ON (g) g, h, x FROM d ORDER BY g, h, x"),
        2
    );
}

#[test]
fn on_expressions_may_appear_in_any_order_among_the_leading_terms() {
    let (_d, c) = open();
    assert_eq!(
        row_count(
            &c,
            "SELECT DISTINCT ON (g, h) g, h, x FROM d ORDER BY h, g, x"
        ),
        2
    );
}

#[test]
fn ordinal_order_by_resolves_before_the_check() {
    let (_d, c) = open();
    // `ORDER BY 1` is the first output column, `g` -- Postgres accepts this.
    assert_eq!(
        row_count(&c, "SELECT DISTINCT ON (g) g, x FROM d ORDER BY 1"),
        2
    );
}

#[test]
fn distinct_on_without_any_order_by_stays_allowed() {
    let (_d, c) = open();
    // With no ORDER BY the surviving row is explicitly unspecified, and
    // Postgres does not reject the query.
    assert_eq!(row_count(&c, "SELECT DISTINCT ON (g) g, x FROM d"), 2);
}

#[test]
fn plain_distinct_is_untouched() {
    let (_d, c) = open();
    assert_eq!(row_count(&c, "SELECT DISTINCT g FROM d ORDER BY g"), 2);
    assert_eq!(row_count(&c, "SELECT DISTINCT g, x FROM d ORDER BY x"), 3);
}
