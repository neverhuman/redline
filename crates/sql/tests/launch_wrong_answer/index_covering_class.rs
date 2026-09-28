//! IDX-EPOCH follow-up: an index-only scan returns the storage class the
//! row holds.
//!
//! INTEGER 2 and REAL 2.0 are one key, so a covering scan takes a whole
//! number's storage class from the column's affinity. That holds only while
//! every stored value has been coerced to that affinity. Three places broke
//! it: a STRICT table's ANY column keeps each value's class (its affinity
//! reads as NUMERIC), `ALTER COLUMN ... TYPE` changed the affinity without
//! converting the stored values, and generated columns were stored and
//! computed without their declared affinity. The covering scan answered
//! INTEGER 2 for a stored REAL 2.0, or the reverse.

use crate::lab::{Lab, Outcome, int, real, text};
use crate::q5_02_partial_update::assert_integrity_ok;

/// Every read shape of `column` through `index`, a scan and the planner.
fn reads_agree(lab: &Lab, table: &str, index: &str, column: &str) {
    for access in [
        format!("{table} INDEXED BY {index}"),
        format!("{table} NOT INDEXED"),
        table.to_owned(),
    ] {
        for order in ["", " DESC"] {
            // Equal keys (2 and 2.0) may come in either order, so this one
            // compares the rows as a multiset.
            lab.assert_same(
                &format!(
                    "SELECT {column} FROM {access} WHERE {column} > -10 ORDER BY {column}{order}"
                ),
                false,
            );
            lab.assert_same(
                &format!(
                    "SELECT {column}, typeof({column}) FROM {access} WHERE {column} >= 2 \
                     ORDER BY {column}{order}, typeof({column})"
                ),
                true,
            );
        }
        lab.assert_same(
            &format!("SELECT {column} FROM {access} WHERE {column} >= 2"),
            false,
        );
    }
}

#[test]
fn strict_any_column_keeps_each_class() {
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE s(x ANY) STRICT; \
         INSERT INTO s VALUES (2.0), (2), (1.5), (3), (-4.0), ('7'); \
         CREATE INDEX s_x ON s(x);",
    );
    lab.assert_rows(
        "SELECT x FROM s INDEXED BY s_x WHERE x >= 2 AND x < 3 ORDER BY typeof(x)",
        &[vec![int(2)], vec![real(2.0)]],
    );
    reads_agree(&lab, "s", "s_x", "x");
    // A non-STRICT column declared ANY has NUMERIC affinity and converts.
    lab.exec_both(
        "CREATE TABLE n(x ANY); INSERT INTO n VALUES (2.0), (2), (1.5); \
         CREATE INDEX n_x ON n(x);",
    );
    reads_agree(&lab, "n", "n_x", "x");
}

#[test]
fn generated_columns_take_their_affinity() {
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE g(a, b REAL GENERATED ALWAYS AS (a) STORED, \
             c INTEGER GENERATED ALWAYS AS (a) VIRTUAL, \
             d TEXT GENERATED ALWAYS AS (a) STORED, \
             e NUMERIC GENERATED ALWAYS AS (a * 1) STORED); \
         INSERT INTO g(a) VALUES (2), (2.0), ('3'), (2.5), (-4); \
         CREATE INDEX g_b ON g(b); CREATE INDEX g_e ON g(e);",
    );
    lab.assert_same(
        "SELECT a, typeof(a), b, typeof(b), c, typeof(c), d, typeof(d), e, typeof(e) \
         FROM g ORDER BY rowid",
        true,
    );
    reads_agree(&lab, "g", "g_b", "b");
    reads_agree(&lab, "g", "g_e", "e");
    lab.exec_both("UPDATE g SET a = 7 WHERE a = 2");
    lab.assert_same(
        "SELECT b, typeof(b), c, typeof(c), d, typeof(d) FROM g ORDER BY rowid",
        true,
    );
    reads_agree(&lab, "g", "g_b", "b");
    assert_integrity_ok(&lab);
}

/// Rows of `sql` on RedlineDB alone.
fn redline_rows(lab: &Lab, sql: &str) -> Vec<Vec<redlinedb_sql::SqlValue>> {
    match lab.rows_redline(sql, &[]) {
        Outcome::Rows(rows) => rows,
        Outcome::Err(err) => panic!("redline failed `{sql}`: {err}"),
    }
}

/// SQLite has no `ALTER COLUMN ... TYPE`: each case builds the table with
/// the new type on the SQLite side, inserts the same values, and requires
/// the same answers from RedlineDB after the ALTER.
#[test]
fn alter_column_type_converts_the_stored_values() {
    let lab = Lab::new();
    let values = "(2.0), (3), ('4'), (2.5), ('x'), (NULL)";
    lab.redline
        .execute(&format!(
            "CREATE TABLE t(x); INSERT INTO t VALUES {values}; CREATE INDEX t_x ON t(x); \
             ALTER TABLE t ALTER COLUMN x TYPE REAL;"
        ))
        .expect("redline alter");
    lab.sqlite
        .execute_batch(&format!(
            "CREATE TABLE t(x REAL); INSERT INTO t VALUES {values}; CREATE INDEX t_x ON t(x);"
        ))
        .expect("sqlite table");
    reads_agree(&lab, "t", "t_x", "x");
    lab.assert_same("SELECT x, typeof(x) FROM t ORDER BY rowid", true);

    // To TEXT: numbers become text, and the index follows.
    let rows = "(1, 2.0), (2, 3), (3, '4'), (4, 2.5), (5, 'x'), (6, NULL)";
    lab.redline
        .execute(&format!(
            "CREATE TABLE u(id INTEGER PRIMARY KEY, x INTEGER); \
             INSERT INTO u VALUES {rows}; CREATE INDEX u_x ON u(x); \
             ALTER TABLE u ALTER COLUMN x TYPE TEXT;"
        ))
        .expect("redline alter");
    lab.sqlite
        .execute_batch(&format!(
            "CREATE TABLE u0(id INTEGER PRIMARY KEY, x INTEGER); INSERT INTO u0 VALUES {rows}; \
             CREATE TABLE u(id INTEGER PRIMARY KEY, x TEXT); \
             INSERT INTO u SELECT id, x FROM u0; CREATE INDEX u_x ON u(x);"
        ))
        .expect("sqlite table");
    lab.assert_same("SELECT id, x, typeof(x) FROM u ORDER BY id", true);
    for access in ["u INDEXED BY u_x", "u NOT INDEXED", "u"] {
        lab.assert_same(&format!("SELECT id FROM {access} WHERE x = '3'"), false);
        lab.assert_same(&format!("SELECT id FROM {access} WHERE x = '2.0'"), false);
        lab.assert_same(
            &format!("SELECT x FROM {access} WHERE x > '' ORDER BY x"),
            true,
        );
    }
    assert_eq!(
        redline_rows(&lab, "SELECT id FROM u INDEXED BY u_x WHERE x = 3"),
        vec![vec![int(2)]]
    );
    assert_integrity_ok(&lab);

    // Back to INTEGER: the text that reads as a number converts again.
    lab.redline
        .execute("ALTER TABLE u ALTER COLUMN x TYPE INTEGER")
        .expect("redline alter back");
    lab.sqlite
        .execute_batch(
            "CREATE TABLE u1(id INTEGER PRIMARY KEY, x INTEGER); \
             INSERT INTO u1 SELECT id, x FROM u; DROP TABLE u; ALTER TABLE u1 RENAME TO u; \
             CREATE INDEX u_x ON u(x);",
        )
        .expect("sqlite table");
    lab.assert_same("SELECT id, x, typeof(x) FROM u ORDER BY id", true);
    reads_agree(&lab, "u", "u_x", "x");
    assert_integrity_ok(&lab);
    assert_eq!(
        redline_rows(&lab, "SELECT typeof(x) FROM u WHERE id = 5"),
        vec![vec![text("text")]]
    );
}

#[test]
fn alter_column_type_keeps_unique_and_strict_rules() {
    let lab = Lab::new();
    // 1 and '1' are two values without affinity and one INTEGER.
    lab.redline
        .execute("CREATE TABLE q(x UNIQUE); INSERT INTO q VALUES (1), ('1');")
        .expect("setup");
    let err = lab
        .redline
        .execute("ALTER TABLE q ALTER COLUMN x TYPE INTEGER")
        .expect_err("two rows convert to one key");
    assert!(
        err.to_string().contains("UNIQUE constraint failed"),
        "{err}"
    );
    // The failed ALTER changed nothing.
    assert_eq!(
        redline_rows(&lab, "SELECT x, typeof(x) FROM q ORDER BY rowid"),
        vec![vec![int(1), text("integer")], vec![text("1"), text("text")]]
    );
    // A STRICT column refuses a value its new type cannot hold.
    lab.redline
        .execute("CREATE TABLE r(x TEXT) STRICT; INSERT INTO r VALUES ('abc');")
        .expect("setup");
    let err = lab
        .redline
        .execute("ALTER TABLE r ALTER COLUMN x TYPE INTEGER")
        .expect_err("'abc' is not an INTEGER");
    assert!(
        err.to_string()
            .to_ascii_lowercase()
            .contains("cannot store"),
        "{err}"
    );
    assert_eq!(
        redline_rows(&lab, "SELECT x, typeof(x) FROM r"),
        vec![vec![text("abc"), text("text")]]
    );
}
