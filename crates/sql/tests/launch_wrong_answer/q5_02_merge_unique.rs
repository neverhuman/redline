//! Q5-02 follow-up: MERGE and foreign-key cascades enforce UNIQUE.
//!
//! A MERGE `WHEN MATCHED THEN UPDATE`, a MERGE `WHEN NOT MATCHED THEN
//! INSERT` and an `ON UPDATE CASCADE` wrote the row and its index entries
//! without probing the UNIQUE constraints first, so each let a second row
//! take a key another row held -- through a partial UNIQUE index, a UNIQUE
//! index or a column's own UNIQUE constraint -- and `PRAGMA integrity_check`
//! then reported the duplicate. SQLite has no MERGE; each case compares
//! with the UPDATE or INSERT the MERGE performs.

use crate::lab::{Lab, error_class, int};
use crate::q5_02_partial_update::{U, assert_integrity_ok, check_u};

/// The error `sql` raises on RedlineDB, as a class.
fn redline_error_class(lab: &Lab, sql: &str) -> &'static str {
    match lab.redline.execute(sql) {
        Ok(_) => panic!("redline accepted `{sql}`"),
        Err(err) => error_class(&err.to_string()),
    }
}

/// The error SQLite raises for the statement the MERGE stands for.
fn sqlite_error_class(lab: &Lab, sql: &str) -> &'static str {
    match lab.sqlite.execute_batch(sql) {
        Ok(()) => panic!("sqlite accepted `{sql}`"),
        Err(err) => error_class(&err.to_string()),
    }
}

#[test]
fn merge_update_into_a_partial_unique_index() {
    let lab = Lab::new();
    lab.exec_both(U);
    lab.exec_both("INSERT INTO u VALUES (1, 9, 1), (2, 9, 0), (3, 10, 0)");
    lab.exec_both("CREATE TABLE s(id INTEGER PRIMARY KEY, flag INTEGER)");
    lab.exec_both("INSERT INTO s VALUES (2, 1)");
    let merge = "MERGE INTO u USING s ON u.id = s.id WHEN MATCHED THEN UPDATE SET flag = s.flag";
    assert_eq!(redline_error_class(&lab, merge), "unique constraint");
    assert_eq!(
        sqlite_error_class(&lab, "UPDATE u SET flag = 1 WHERE id = 2"),
        "unique constraint"
    );
    check_u(&lab, &[9, 10]);
    // A MERGE whose update does not collide goes through.
    lab.exec_both("UPDATE s SET id = 3");
    lab.redline.execute(merge).expect("merge into a free key");
    lab.sqlite
        .execute_batch("UPDATE u SET flag = 1 WHERE id = 3")
        .expect("sqlite update");
    check_u(&lab, &[9, 10]);
}

#[test]
fn merge_enforces_unique_indexes_and_constraints() {
    let lab = Lab::new();
    lab.exec_both(
        "CREATE TABLE w(id INTEGER PRIMARY KEY, k INTEGER UNIQUE, v INTEGER); \
         CREATE UNIQUE INDEX w_v ON w(v); \
         INSERT INTO w VALUES (1, 9, 100), (2, 8, 200); \
         CREATE TABLE s(id INTEGER PRIMARY KEY, k INTEGER, v INTEGER);",
    );
    // Matched update onto another row's UNIQUE column value.
    lab.exec_both("INSERT INTO s VALUES (2, 9, 300)");
    assert_eq!(
        redline_error_class(
            &lab,
            "MERGE INTO w USING s ON w.id = s.id WHEN MATCHED THEN UPDATE SET k = s.k",
        ),
        "unique constraint"
    );
    assert_eq!(
        sqlite_error_class(&lab, "UPDATE w SET k = 9 WHERE id = 2"),
        "unique constraint"
    );
    // Matched update onto another row's UNIQUE index key.
    lab.exec_both("UPDATE s SET v = 100");
    assert_eq!(
        redline_error_class(
            &lab,
            "MERGE INTO w USING s ON w.id = s.id WHEN MATCHED THEN UPDATE SET v = s.v",
        ),
        "unique constraint"
    );
    // Not-matched insert of a key a row holds: `k` (a column constraint)
    // or `v` (a UNIQUE index). A MERGE source is a table.
    lab.exec_both(
        "CREATE TABLE s3(id INTEGER PRIMARY KEY, k INTEGER, v INTEGER); \
         INSERT INTO s3 VALUES (3, 8, 400); \
         CREATE TABLE s4(id INTEGER PRIMARY KEY, k INTEGER, v INTEGER); \
         INSERT INTO s4 VALUES (4, 7, 200); \
         CREATE TABLE s5(id INTEGER PRIMARY KEY, k INTEGER, v INTEGER); \
         INSERT INTO s5 VALUES (5, 5, 500);",
    );
    let insert_from = |source: &str| {
        format!(
            "MERGE INTO w USING {source} ON w.id = {source}.id WHEN NOT MATCHED THEN \
             INSERT (id, k, v) VALUES ({source}.id, {source}.k, {source}.v)"
        )
    };
    for source in ["s3", "s4"] {
        assert_eq!(
            redline_error_class(&lab, &insert_from(source)),
            "unique constraint"
        );
    }
    for insert in [
        "INSERT INTO w VALUES (3, 8, 400)",
        "INSERT INTO w VALUES (4, 7, 200)",
    ] {
        assert_eq!(sqlite_error_class(&lab, insert), "unique constraint");
    }
    lab.assert_same("SELECT id, k, v FROM w ORDER BY id", true);
    assert_integrity_ok(&lab);
    // A not-matched insert of free keys goes through.
    lab.redline
        .execute(&insert_from("s5"))
        .expect("merge insert");
    lab.sqlite
        .execute_batch("INSERT INTO w VALUES (5, 5, 500)")
        .expect("sqlite insert");
    lab.assert_same("SELECT id, k, v FROM w ORDER BY id", true);
    assert_integrity_ok(&lab);
}

#[test]
fn cascade_into_a_unique_child_key() {
    let lab = Lab::new();
    // Child 2 holds key 7 with no parent (foreign keys are off while it is
    // written), so the cascade below can move child 1 onto it.
    lab.exec_both("PRAGMA foreign_keys = OFF");
    lab.exec_both(
        "CREATE TABLE p(id INTEGER PRIMARY KEY, code INTEGER); \
         CREATE UNIQUE INDEX p_code ON p(code); \
         CREATE TABLE c(id INTEGER PRIMARY KEY, \
             k INTEGER REFERENCES p(code) ON UPDATE CASCADE, flag INTEGER); \
         CREATE UNIQUE INDEX cu ON c(k) WHERE flag = 1; \
         CREATE UNIQUE INDEX cv ON c(id, k); \
         INSERT INTO p VALUES (1, 5), (2, 6); \
         INSERT INTO c VALUES (1, 5, 1), (2, 7, 1), (3, 6, 0);",
    );
    lab.exec_both("PRAGMA foreign_keys = ON");
    let update = "UPDATE p SET code = 7 WHERE id = 1";
    assert_eq!(redline_error_class(&lab, update), "unique constraint");
    assert_eq!(sqlite_error_class(&lab, update), "unique constraint");
    lab.assert_same("SELECT id, k, flag FROM c ORDER BY id", true);
    lab.assert_same("SELECT id, code FROM p ORDER BY id", true);
    assert_integrity_ok(&lab);
    // A cascade onto a free key goes through, into and out of the index.
    lab.exec_both("UPDATE p SET code = 8 WHERE id = 1");
    lab.exec_both("UPDATE p SET code = 9 WHERE id = 2");
    lab.assert_rows(
        "SELECT id, k FROM c ORDER BY id",
        &[
            vec![int(1), int(8)],
            vec![int(2), int(7)],
            vec![int(3), int(9)],
        ],
    );
    assert_integrity_ok(&lab);
}
