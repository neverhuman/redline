//! Q5-05 follow-up: comparison affinity of views, attached tables, CTEs,
//! subqueries and table-valued functions.
//!
//! These sources reach the executor as synthetic tables, and comparison
//! affinity treated every one of their columns as having no affinity, so a
//! TEXT column compared with them converted the other operand to TEXT:
//! `y IN (SELECT value FROM json_each('[5]'))` found TEXT '5'. SQLite gives
//! a table-valued function's columns (declared without a type) BLOB
//! affinity, and a view or subquery column the affinity of its defining
//! expression, so a typeless column compares without conversion, and an
//! attached table's columns keep their declared affinity.

use crate::lab::{Lab, Outcome, int};

fn setup(lab: &Lab) {
    lab.exec_both(
        "CREATE TABLE t(id INTEGER PRIMARY KEY, y TEXT); \
         INSERT INTO t VALUES (1, '5'), (2, 'abc'), (3, '7'); \
         CREATE TABLE u(id INTEGER PRIMARY KEY, w); \
         INSERT INTO u VALUES (1, 5), (2, '7'), (3, 7.0); \
         CREATE TABLE n(id INTEGER PRIMARY KEY, x INTEGER); \
         INSERT INTO n VALUES (1, 5), (2, 7); \
         CREATE VIEW v AS SELECT w FROM u; \
         CREATE VIEW vn AS SELECT x FROM n; \
         CREATE VIEW vl AS SELECT 5 AS l;",
    );
}

#[test]
fn table_valued_function_columns_have_blob_affinity() {
    let lab = Lab::new();
    setup(&lab);
    for sql in [
        "SELECT count(*) FROM t WHERE y IN (SELECT value FROM json_each('[5]'))",
        "SELECT count(*) FROM t WHERE y IN (SELECT value FROM json_each('[5, \"7\"]'))",
        "SELECT count(*) FROM t, json_each('[5, 7]') j WHERE t.y = j.value",
        "SELECT count(*) FROM n, json_each('[\"5\", \"x\"]') j WHERE n.x = j.value",
        "SELECT count(*) FROM t WHERE y = (SELECT value FROM json_each('[7]'))",
    ] {
        lab.assert_same(sql, true);
    }
    // generate_series is not in the bundled SQLite; sqlite3 3.53.1 answers
    // 0 (`value` is typeless, so '5' and 5 differ).
    match lab.rows_redline(
        "SELECT count(*) FROM t, generate_series(1, 9) WHERE t.y = value",
        &[],
    ) {
        Outcome::Rows(rows) => assert_eq!(rows, vec![vec![int(0)]]),
        Outcome::Err(err) => panic!("generate_series: {err}"),
    }
}

#[test]
fn view_and_subquery_columns_take_the_defining_affinity() {
    let lab = Lab::new();
    setup(&lab);
    for sql in [
        // A view column over a typeless column is BLOB: no conversion.
        "SELECT count(*) FROM t JOIN v ON t.y = v.w",
        "SELECT t.id, v.w FROM t, v WHERE t.y = v.w",
        // Over an INTEGER column it is INTEGER: '5' converts.
        "SELECT x FROM vn WHERE x = '5'",
        "SELECT t.id, vn.x FROM t JOIN vn ON t.y = vn.x",
        "SELECT count(*) FROM vn WHERE x IN ('5', '7')",
        // A literal has no affinity: the TEXT side converts it.
        "SELECT t.id FROM t JOIN vl ON t.y = vl.l",
        // FROM-subqueries and CTEs follow the same rules.
        "SELECT count(*) FROM t JOIN (SELECT w FROM u) s ON t.y = s.w",
        "SELECT t.id FROM t JOIN (SELECT x FROM n) s ON t.y = s.x",
        "WITH c AS (SELECT w FROM u) SELECT count(*) FROM t JOIN c ON t.y = c.w",
        "WITH c AS (SELECT x FROM n) SELECT t.id FROM t JOIN c ON t.y = c.x",
        "WITH c AS (SELECT x FROM n) SELECT x FROM c WHERE x = '5'",
        "WITH c AS (SELECT x + 0 AS e FROM n) SELECT t.id FROM t JOIN c ON t.y = c.e",
        "SELECT s.x FROM (SELECT x FROM n) s WHERE s.x = '7'",
        "SELECT column1 FROM (VALUES (5), (7)) WHERE column1 = '5'",
        "SELECT t.id FROM t JOIN (VALUES (5), (7)) v ON t.y = v.column1",
    ] {
        lab.assert_same(sql, false);
    }
}

#[test]
fn attached_table_columns_keep_their_declared_affinity() {
    let lab = Lab::new();
    let dir = tempfile::tempdir().expect("scratch dir");
    let aux = dir.path().join("aux.db");
    let sqlite_aux = dir.path().join("aux.sqlite");
    lab.redline
        .execute(&format!("ATTACH '{}' AS aux", aux.display()))
        .expect("redline attach");
    lab.sqlite
        .execute_batch(&format!("ATTACH '{}' AS aux", sqlite_aux.display()))
        .expect("sqlite attach");
    lab.exec_both(
        "CREATE TABLE aux.an(id INTEGER PRIMARY KEY, x INTEGER); \
         INSERT INTO aux.an VALUES (1, 5), (2, 7); \
         CREATE TABLE aux.au(id INTEGER PRIMARY KEY, w); \
         INSERT INTO aux.au VALUES (1, 5), (2, '7'); \
         CREATE TABLE t(id INTEGER PRIMARY KEY, y TEXT); \
         INSERT INTO t VALUES (1, '5'), (2, '7');",
    );
    for sql in [
        "SELECT x FROM aux.an WHERE x = '5'",
        "SELECT t.id, an.x FROM t JOIN aux.an an ON t.y = an.x",
        "SELECT t.id, au.w FROM t JOIN aux.au au ON t.y = au.w",
    ] {
        lab.assert_same(sql, false);
    }
}
