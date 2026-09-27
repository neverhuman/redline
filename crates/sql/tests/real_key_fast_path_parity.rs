//! Launch P1-real-pk: the unindexed conflict, rowid-scan, ALTER and
//! foreign-key fast paths must give SQLite's answer when a key arrives as
//! REAL or TEXT instead of INTEGER.
//!
//! Inline `PRIMARY KEY` / `UNIQUE` constraints have no physical B-tree, so
//! their conflict check is the heap scan whose fast paths are under test.
//! Every statement is compared with bundled SQLite by storage class.

#[path = "real_key_fast_path/lab.rs"]
mod lab;

use lab::{Lab, Outcome, Val, datatype_err, fk_err, unique_err};

fn rows(rows: &[&[Val]]) -> Outcome {
    Outcome::Rows(rows.iter().map(|row| row.to_vec()).collect())
}

fn int(v: i64) -> Val {
    Val::Int(v)
}

fn real(v: f64) -> Val {
    Val::Real(v.to_bits())
}

fn text(v: &str) -> Val {
    Val::Text(v.to_owned())
}

fn pk_table(lab: &Lab) {
    lab.script(&[
        "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)",
        "INSERT INTO t(id, v) VALUES (5, 'a'), (6, 'b')",
    ]);
}

const PK_DUMP: &str = "SELECT id, typeof(id), v FROM t ORDER BY id";

// -- INTEGER PRIMARY KEY (rowid alias) ----------------------------------------

#[test]
fn integer_pk_rejects_real_duplicate_of_integer_key() {
    let lab = Lab::new();
    pk_table(&lab);
    lab.expect("INSERT INTO t(id, v) VALUES (5.0, 'x')", unique_err());
    lab.expect("INSERT INTO t(id, v) VALUES ('5', 'x')", unique_err());
    lab.expect("INSERT INTO t(id, v) VALUES ('5.0', 'x')", unique_err());
    lab.expect("INSERT INTO t(id, v) VALUES (' 5 ', 'x')", unique_err());
    lab.expect("INSERT INTO t(id, v) VALUES (5.5, 'x')", datatype_err());
    lab.expect("INSERT INTO t(id, v) VALUES ('five', 'x')", datatype_err());
    lab.step(PK_DUMP);
}

#[test]
fn integer_pk_rejects_integer_duplicate_of_real_input() {
    let lab = Lab::new();
    lab.script(&[
        "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)",
        "INSERT INTO t(id, v) VALUES (7.0, 'a')",
    ]);
    lab.expect("INSERT INTO t(id, v) VALUES (7, 'x')", unique_err());
    lab.expect(PK_DUMP, rows(&[&[int(7), text("integer"), text("a")]]));
}

#[test]
fn integer_pk_rejects_real_duplicate_inside_one_statement_and_transaction() {
    let lab = Lab::new();
    lab.script(&["CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)"]);
    lab.expect(
        "INSERT INTO t(id, v) VALUES (5, 'a'), (5.0, 'b')",
        unique_err(),
    );
    lab.expect("SELECT count(*) FROM t", rows(&[&[int(0)]]));
    // An uncommitted row in the same transaction is still a conflict.
    // (A failed statement poisons a RedlineDB transaction, so end with
    // ROLLBACK rather than COMMIT; that deviation is tracked elsewhere.)
    lab.script(&["BEGIN", "INSERT INTO t(id, v) VALUES (9, 'a')"]);
    lab.expect("INSERT INTO t(id, v) VALUES (9.0, 'b')", unique_err());
    lab.expect("ROLLBACK", Outcome::Done);
    lab.expect("SELECT count(*) FROM t", rows(&[&[int(0)]]));
}

#[test]
fn integer_pk_update_to_real_duplicate_is_rejected() {
    let lab = Lab::new();
    pk_table(&lab);
    lab.expect("UPDATE t SET id = 5.0 WHERE id = 6", unique_err());
    lab.expect("UPDATE t SET id = '5' WHERE id = 6", unique_err());
    lab.expect("UPDATE t SET rowid = 5.0 WHERE id = 6", unique_err());
    lab.expect("UPDATE t SET id = 6.0 WHERE id = 6", Outcome::Done);
    lab.expect("UPDATE t SET id = 8.0 WHERE id = 6", Outcome::Done);
    lab.expect(
        PK_DUMP,
        rows(&[
            &[int(5), text("integer"), text("a")],
            &[int(8), text("integer"), text("b")],
        ]),
    );
}

#[test]
fn integer_pk_rowid_pseudo_column_and_alias_share_one_key() {
    let lab = Lab::new();
    lab.script(&[
        "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)",
        "INSERT INTO t(rowid, v) VALUES (5, 'a')",
    ]);
    lab.expect("INSERT INTO t(id, v) VALUES (5.0, 'x')", unique_err());
    lab.expect("INSERT INTO t(rowid, v) VALUES (5.0, 'x')", unique_err());
    lab.step(PK_DUMP);
}

#[test]
fn integer_pk_conflict_resolution_with_real_key() {
    let lab = Lab::new();
    pk_table(&lab);
    lab.expect(
        "INSERT OR IGNORE INTO t(id, v) VALUES (5.0, 'ignored')",
        Outcome::Done,
    );
    lab.expect(
        "INSERT OR REPLACE INTO t(id, v) VALUES (5.0, 'replaced')",
        Outcome::Done,
    );
    lab.expect(
        "INSERT INTO t(id, v) VALUES (6.0, 'nothing') ON CONFLICT(id) DO NOTHING",
        Outcome::Done,
    );
    lab.expect(
        "INSERT INTO t(id, v) VALUES (6.0, 'upserted') ON CONFLICT(id) DO UPDATE SET v = excluded.v",
        Outcome::Done,
    );
    lab.expect(
        "INSERT INTO t(id, v) VALUES ('6', 'text-key') ON CONFLICT DO UPDATE SET v = excluded.v || typeof(excluded.id)",
        Outcome::Done,
    );
    lab.expect(
        PK_DUMP,
        rows(&[
            &[int(5), text("integer"), text("replaced")],
            &[int(6), text("integer"), text("text-keyinteger")],
        ]),
    );
}

#[test]
fn integer_pk_upsert_update_moving_key_onto_real_duplicate_is_rejected() {
    let lab = Lab::new();
    pk_table(&lab);
    lab.expect(
        "INSERT INTO t(id, v) VALUES (6, 'x') ON CONFLICT(id) DO UPDATE SET id = 5.0",
        unique_err(),
    );
    lab.step(PK_DUMP);
}

#[test]
fn integer_pk_delete_then_real_key_reinsert() {
    let lab = Lab::new();
    pk_table(&lab);
    lab.expect("DELETE FROM t WHERE id = 5.0", Outcome::Done);
    lab.expect("INSERT INTO t(id, v) VALUES (5.0, 'again')", Outcome::Done);
    lab.expect("INSERT INTO t(id, v) VALUES (5, 'dup')", unique_err());
    lab.step(PK_DUMP);
}

/// A REAL past the i64 range survives integer affinity. It used to become
/// a u64 rowid, and the rowid-only conflict check skipped it, so the
/// second insert silently replaced the first row.
#[test]
fn integer_pk_out_of_range_real_key_is_rejected() {
    let lab = Lab::new();
    lab.script(&["CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)"]);
    lab.expect("INSERT INTO t(id, v) VALUES (1e19, 'a')", datatype_err());
    lab.expect("INSERT INTO t(id, v) VALUES (1e19, 'b')", datatype_err());
    lab.expect("INSERT INTO t(id, v) VALUES ('1e19', 'c')", datatype_err());
    lab.expect("INSERT INTO t(id, v) VALUES (1e300, 'd')", datatype_err());
    lab.expect(
        "INSERT OR REPLACE INTO t(id, v) VALUES (1e19, 'e')",
        datatype_err(),
    );
    lab.expect("SELECT count(*) FROM t", rows(&[&[int(0)]]));
    lab.script(&["INSERT INTO t(id, v) VALUES (1, 'kept')"]);
    lab.expect("UPDATE t SET id = 1e19", datatype_err());
    lab.expect(PK_DUMP, rows(&[&[int(1), text("integer"), text("kept")]]));
}

// -- Unindexed UNIQUE columns (unique-key-columns fast path) ----------------

fn unique_parity(declared: &str, first: &str, probes: &[&str]) {
    let lab = Lab::new();
    lab.script(&[
        &format!("CREATE TABLE u(id INTEGER PRIMARY KEY, k {declared} UNIQUE, note TEXT)"),
        &format!("INSERT INTO u(k, note) VALUES ({first}, 'first')"),
    ]);
    for probe in probes {
        lab.step(&format!("INSERT INTO u(k, note) VALUES ({probe}, 'probe')"));
    }
    lab.step("SELECT k, typeof(k), note FROM u ORDER BY id");
}

#[test]
fn unique_integer_column_treats_one_and_one_point_zero_as_equal() {
    let lab = Lab::new();
    lab.script(&[
        "CREATE TABLE u(id INTEGER PRIMARY KEY, k INTEGER UNIQUE, note TEXT)",
        "INSERT INTO u(k, note) VALUES (1, 'first')",
    ]);
    lab.expect("INSERT INTO u(k, note) VALUES (1.0, 'x')", unique_err());
    lab.expect("INSERT INTO u(k, note) VALUES ('1', 'x')", unique_err());
    lab.expect("INSERT INTO u(k, note) VALUES (1.5, 'y')", Outcome::Done);
    lab.expect("INSERT INTO u(k, note) VALUES ('1.5', 'y')", unique_err());
}

#[test]
fn unique_column_affinity_matrix_matches_sqlite() {
    let probes = ["1", "1.0", "'1'", "'1.0'", "' 1'", "x'01'"];
    for declared in ["INTEGER", "REAL", "NUMERIC", "TEXT", "BLOB", ""] {
        for first in ["1", "1.0", "'1'"] {
            unique_parity(declared, first, &probes);
        }
    }
}

#[test]
fn unique_untyped_column_numeric_values_collide() {
    let lab = Lab::new();
    lab.script(&[
        "CREATE TABLE u(id INTEGER PRIMARY KEY, k UNIQUE)",
        "INSERT INTO u(k) VALUES (1)",
    ]);
    lab.expect("INSERT INTO u(k) VALUES (1.0)", unique_err());
    lab.expect("INSERT INTO u(k) VALUES ('1')", Outcome::Done);
    lab.expect("INSERT INTO u(k) VALUES ('1')", unique_err());
}

#[test]
fn unique_composite_key_with_mixed_numeric_storage() {
    let lab = Lab::new();
    lab.script(&[
        "CREATE TABLE u(id INTEGER PRIMARY KEY, a, b REAL, c TEXT, UNIQUE(a, b, c))",
        "INSERT INTO u(a, b, c) VALUES (1, 2, 'x')",
    ]);
    lab.expect(
        "INSERT INTO u(a, b, c) VALUES (1.0, 2.0, 'x')",
        unique_err(),
    );
    lab.expect(
        "INSERT INTO u(a, b, c) VALUES (1.0, '2', 'x')",
        unique_err(),
    );
    lab.expect("INSERT INTO u(a, b, c) VALUES ('1', 2, 'x')", Outcome::Done);
    lab.expect("INSERT INTO u(a, b, c) VALUES (1, 2, 'X')", Outcome::Done);
    lab.step("SELECT a, typeof(a), b, typeof(b), c FROM u ORDER BY id");
}

#[test]
fn unique_column_update_and_resolution_with_real_value() {
    let lab = Lab::new();
    lab.script(&[
        "CREATE TABLE u(id INTEGER PRIMARY KEY, k INTEGER UNIQUE, note TEXT)",
        "INSERT INTO u(id, k, note) VALUES (1, 1, 'a'), (2, 2, 'b')",
    ]);
    lab.expect("UPDATE u SET k = 1.0 WHERE id = 2", unique_err());
    lab.expect("UPDATE u SET k = '1' WHERE id = 2", unique_err());
    lab.expect(
        "INSERT OR IGNORE INTO u(id, k, note) VALUES (3, 1.0, 'ignored')",
        Outcome::Done,
    );
    lab.expect(
        "INSERT INTO u(id, k, note) VALUES (4, 2.0, 'nothing') ON CONFLICT(k) DO NOTHING",
        Outcome::Done,
    );
    lab.expect(
        "INSERT INTO u(id, k, note) VALUES (5, 2.0, 'upd') ON CONFLICT(k) DO UPDATE SET note = excluded.note",
        Outcome::Done,
    );
    lab.expect(
        "INSERT OR REPLACE INTO u(id, k, note) VALUES (6, 1.0, 'replaced')",
        Outcome::Done,
    );
    lab.expect(
        "SELECT id, k, typeof(k), note FROM u ORDER BY id",
        rows(&[
            &[int(2), int(2), text("integer"), text("upd")],
            &[int(6), int(1), text("integer"), text("replaced")],
        ]),
    );
}

#[test]
fn unique_stored_generated_column_key_sees_computed_values() {
    let lab = Lab::new();
    lab.script(&[
        "CREATE TABLE g(id INTEGER PRIMARY KEY, a, k GENERATED ALWAYS AS (a * 2) STORED UNIQUE)",
    ]);
    lab.expect("INSERT INTO g(a) VALUES (1)", Outcome::Done);
    lab.expect("INSERT INTO g(a) VALUES (1.0)", unique_err());
    lab.expect("INSERT INTO g(a) VALUES (2)", Outcome::Done);
    lab.expect("UPDATE g SET a = 1.0 WHERE a = 2", unique_err());
    lab.step("SELECT a, k, typeof(k) FROM g ORDER BY id");
}

#[test]
fn unique_check_sees_default_of_column_added_after_rows() {
    let lab = Lab::new();
    lab.script(&[
        "CREATE TABLE a(id INTEGER PRIMARY KEY, k INTEGER, UNIQUE(k))",
        "INSERT INTO a(id, k) VALUES (1, 10)",
        "ALTER TABLE a ADD COLUMN extra REAL DEFAULT 2",
    ]);
    lab.expect(
        "INSERT INTO a(id, k, extra) VALUES (2, 10.0, 3)",
        unique_err(),
    );
    lab.expect(
        "INSERT INTO a(id, k, extra) VALUES (2, 11, 3)",
        Outcome::Done,
    );
    lab.expect(
        "SELECT id, k, extra, typeof(extra) FROM a ORDER BY id",
        rows(&[
            &[int(1), int(10), real(2.0), text("real")],
            &[int(2), int(11), real(3.0), text("real")],
        ]),
    );
}

// -- Foreign keys with REAL keys -------------------------------------------

#[test]
fn fk_child_real_key_finds_integer_parent() {
    let lab = Lab::new();
    lab.script(&[
        "PRAGMA foreign_keys = ON",
        "CREATE TABLE p(id INTEGER PRIMARY KEY)",
        "CREATE TABLE c(id INTEGER PRIMARY KEY, pid INTEGER REFERENCES p(id))",
        "INSERT INTO p(id) VALUES (5)",
    ]);
    lab.expect("INSERT INTO c(pid) VALUES (5.0)", Outcome::Done);
    lab.expect("INSERT INTO c(pid) VALUES ('5')", Outcome::Done);
    lab.expect("INSERT INTO c(pid) VALUES (5.5)", fk_err());
    lab.expect("DELETE FROM p WHERE id = 5.0", fk_err());
    lab.step("SELECT pid, typeof(pid) FROM c ORDER BY id");
}

#[test]
fn fk_real_parent_key_matches_integer_child() {
    let lab = Lab::new();
    lab.script(&[
        "PRAGMA foreign_keys = ON",
        "CREATE TABLE p(k REAL UNIQUE)",
        "CREATE TABLE c(id INTEGER PRIMARY KEY, pk INTEGER REFERENCES p(k))",
        "INSERT INTO p(k) VALUES (5)",
    ]);
    lab.expect(
        "SELECT k, typeof(k) FROM p",
        rows(&[&[real(5.0), text("real")]]),
    );
    lab.expect("INSERT INTO c(pk) VALUES (5)", Outcome::Done);
    lab.expect("INSERT INTO c(pk) VALUES (5.0)", Outcome::Done);
    lab.expect("INSERT INTO c(pk) VALUES (6)", fk_err());
    lab.expect("DELETE FROM p WHERE k = 5", fk_err());
    lab.step("SELECT pk, typeof(pk) FROM c ORDER BY id");
}

#[test]
fn fk_untyped_parent_key_stored_as_real() {
    let lab = Lab::new();
    lab.script(&[
        "PRAGMA foreign_keys = ON",
        "CREATE TABLE p(k UNIQUE)",
        "CREATE TABLE c(id INTEGER PRIMARY KEY, pk REFERENCES p(k))",
        "INSERT INTO p(k) VALUES (5.0)",
    ]);
    lab.expect("INSERT INTO c(pk) VALUES (5)", Outcome::Done);
    lab.expect("INSERT INTO c(pk) VALUES ('5')", fk_err());
    lab.expect("DELETE FROM p", fk_err());
    lab.step("SELECT pk, typeof(pk) FROM c ORDER BY id");
}

// -- ALTER TABLE row reuse ------------------------------------------------------

#[test]
fn drop_column_keeps_key_storage_classes_and_conflicts() {
    let lab = Lab::new();
    lab.script(&[
        "CREATE TABLE t(id INTEGER PRIMARY KEY, junk TEXT, k UNIQUE, r REAL)",
        "INSERT INTO t(id, junk, k, r) VALUES (1, 'x', 1.0, 3), (2, 'y', '2', 4.5)",
        "ALTER TABLE t ADD COLUMN late REAL DEFAULT 9",
        "ALTER TABLE t DROP COLUMN junk",
    ]);
    lab.expect(
        "SELECT id, k, typeof(k), r, typeof(r), late, typeof(late) FROM t ORDER BY id",
        rows(&[
            &[
                int(1),
                real(1.0),
                text("real"),
                real(3.0),
                text("real"),
                real(9.0),
                text("real"),
            ],
            &[
                int(2),
                text("2"),
                text("text"),
                real(4.5),
                text("real"),
                real(9.0),
                text("real"),
            ],
        ]),
    );
    lab.expect("INSERT INTO t(id, k) VALUES (1.0, 7)", unique_err());
    lab.expect("INSERT INTO t(id, k) VALUES (3, 1)", unique_err());
    lab.expect("INSERT INTO t(id, k) VALUES (3, 2)", Outcome::Done);
    lab.step("SELECT id, k, typeof(k) FROM t ORDER BY id");
}
