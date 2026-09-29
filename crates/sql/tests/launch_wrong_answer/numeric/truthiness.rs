//! Numeric-prefix truthiness: SQLite reads a TEXT or BLOB boolean through its
//! longest numeric prefix, in expressions, trigger WHEN clauses, partial
//! indexes and CHECK constraints.

use super::*;

/// TEXT and BLOB values whose truth SQLite reads from the numeric prefix.
const TRUTH_VALUES: &[&str] = &[
    "'1abc'",
    "x'31ff'",
    "'1e'",
    "'.5x'",
    "' 2 apples'",
    "'-3x'",
    "'abc'",
    "'0x1'",
    "'0.0x'",
    "''",
    "'inf'",
    "'nan'",
    "'1'",
    "'0'",
    "x''",
    "x'30'",
    "'  +7'",
    "'1e400'",
];

#[test]
fn numeric_prefix_truthiness() {
    let lab = Lab::new();
    for value in TRUTH_VALUES {
        lab.assert_same(&format!("SELECT CASE WHEN {value} THEN 1 ELSE 0 END"), true);
        lab.assert_same(
            &format!("SELECT NOT {value}, {value} AND 1, {value} OR 0, 1 AND {value}"),
            true,
        );
        lab.assert_same(
            &format!("SELECT CASE WHEN v THEN 1 ELSE 0 END, NOT v FROM (SELECT {value} AS v)"),
            true,
        );
    }
    lab.assert_rows(
        "SELECT CASE WHEN '1abc' THEN 1 ELSE 0 END, CASE WHEN x'31ff' THEN 1 ELSE 0 END, \
         CASE WHEN '1e' THEN 1 ELSE 0 END, CASE WHEN '.5x' THEN 1 ELSE 0 END, NOT '1abc'",
        &[vec![int(1), int(1), int(1), int(1), int(0)]],
    );

    lab.exec_both("CREATE TABLE tv(id INTEGER PRIMARY KEY, v);");
    for (i, value) in TRUTH_VALUES.iter().enumerate() {
        lab.exec_both(&format!("INSERT INTO tv VALUES ({}, {value});", i + 1));
    }
    lab.assert_same("SELECT id FROM tv WHERE v ORDER BY id", true);
    lab.assert_same("SELECT id FROM tv WHERE NOT v ORDER BY id", true);
    lab.assert_same("SELECT id FROM tv WHERE v AND id > 0 ORDER BY id", true);
    lab.assert_same(
        "SELECT id, CASE WHEN v THEN 't' ELSE 'f' END FROM tv ORDER BY id",
        true,
    );
    lab.assert_same("SELECT count(*) FILTER (WHERE v) FROM tv", true);
}

#[test]
fn numeric_prefix_truthiness_in_trigger_when() {
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE src(v); CREATE TABLE fired(v); \
         CREATE TRIGGER tr AFTER INSERT ON src WHEN NEW.v \
         BEGIN INSERT INTO fired VALUES (NEW.v); END;",
    );
    for value in TRUTH_VALUES {
        lab.step_both(&format!("INSERT INTO src VALUES ({value});"));
    }
    lab.assert_same("SELECT v, typeof(v) FROM fired ORDER BY rowid", true);
    lab.assert_rows(
        "SELECT count(*) FROM fired WHERE v = '1abc'",
        &[vec![int(1)]],
    );
}

#[test]
fn numeric_prefix_truthiness_in_partial_index() {
    let lab = Lab::new();
    lab.exec_both("CREATE TABLE p(a INTEGER, flag); CREATE INDEX pi ON p(a) WHERE flag;");
    for (i, value) in TRUTH_VALUES.iter().enumerate() {
        lab.exec_both(&format!("INSERT INTO p VALUES ({}, {value});", i + 1));
    }
    lab.indexed_vs_scan("p", "pi", "SELECT a FROM {access} WHERE flag AND a > 0");
    lab.assert_rows(
        "SELECT a FROM p INDEXED BY pi WHERE flag AND a = 1",
        &[vec![int(1)]],
    );
    // Deleting a member and inserting new rows keep the index in step.
    // (UPDATE moving a row across the predicate is launch Q5-02, a separate
    // partial-index defect, so it is not exercised here.)
    lab.exec_both("DELETE FROM p WHERE a = 2; DELETE FROM p WHERE a = 7;");
    lab.exec_both("INSERT INTO p VALUES (100, '9lives'), (101, 'zzz'), (102, x'2d3161');");
    lab.indexed_vs_scan("p", "pi", "SELECT a FROM {access} WHERE flag AND a > 0");
}

#[test]
fn check_constraints_use_exact_compare_and_prefix_truth() {
    // CHECK expressions run in the kernel evaluator, which had its own
    // lossy INTEGER/REAL compare and "non-empty TEXT is true" rule.
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE ck_cmp(x CHECK (x > 9007199254740992.0)); \
         CREATE TABLE ck_not(v, CHECK (NOT v)); \
         CREATE TABLE ck_and(v, w, CHECK (v AND w));",
    );
    lab.step_both("INSERT INTO ck_cmp VALUES (9007199254740993)");
    lab.step_both("INSERT INTO ck_cmp VALUES (9007199254740992)");
    for value in TRUTH_VALUES {
        lab.step_both(&format!("INSERT INTO ck_not VALUES ({value})"));
        lab.step_both(&format!("INSERT INTO ck_and VALUES ({value}, 1)"));
    }
    lab.assert_same("SELECT x, typeof(x) FROM ck_cmp ORDER BY rowid", true);
    lab.assert_same("SELECT v, typeof(v) FROM ck_not ORDER BY rowid", true);
    lab.assert_same("SELECT v, typeof(v) FROM ck_and ORDER BY rowid", true);
}
