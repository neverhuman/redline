//! Launch P1-real-pk review follow-ups: key comparisons that the first pass
//! left outside SQLite's answer. Every statement is compared with bundled
//! SQLite by storage class.

#[path = "real_key_fast_path/lab.rs"]
mod lab;

use lab::{Lab, Outcome, Val};
use redlinedb_sql::{Database, DbOptions, Step};

fn rows(rows: &[&[Val]]) -> Outcome {
    Outcome::Rows(rows.iter().map(|row| row.to_vec()).collect())
}

fn int(v: i64) -> Val {
    Val::Int(v)
}

fn text(v: &str) -> Val {
    Val::Text(v.to_owned())
}

// -- rowid equality fast path ------------------------------------------------

/// Values that reach `WHERE id = <value>` against an INTEGER PRIMARY KEY and
/// equal no rowid: negative, fractional, out of range, NULL, non-numeric
/// text, a blob, or numeric text that is not a whole non-negative number.
const NO_ROWID_PROBES: [&str; 11] = [
    "5.5", "-1", "'-1'", "'5.5'", "'x'", "'5x'", "x'05'", "1e100", "-0.5", "NULL", "'1e100'",
];

/// Numeric text that SQLite compares with the rowid as its number. RedlineDB
/// does not apply comparison affinity in WHERE yet, so it refuses these
/// rather than answer with no row.
const NUMERIC_TEXT_PROBES: [&str; 4] = ["'5'", "' 5 '", "'5.0'", "'+5'"];

fn key_table(lab: &Lab) {
    lab.script(&[
        "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)",
        "INSERT INTO t(id, v) VALUES (5, 'a'), (6, 'b')",
    ]);
}

#[test]
fn rowid_equality_answers_values_that_equal_no_rowid_like_sqlite() {
    let lab = Lab::new();
    key_table(&lab);
    lab.expect(
        "SELECT id, v FROM t WHERE id = 5.0",
        rows(&[&[int(5), text("a")]]),
    );
    lab.expect("SELECT id, v FROM t WHERE id = 5.5", rows(&[]));
    for probe in NO_ROWID_PROBES {
        for sql in [
            format!("SELECT id, v FROM t WHERE id = {probe}"),
            format!("SELECT id, v FROM t WHERE {probe} = id"),
            format!("SELECT id, v FROM t WHERE t.rowid = {probe}"),
            format!("SELECT count(*) FROM t WHERE id = {probe}"),
        ] {
            lab.step(&sql);
        }
        lab.expect(
            &format!("UPDATE t SET v = 'u' WHERE id = {probe}"),
            Outcome::Done,
        );
        lab.expect(&format!("DELETE FROM t WHERE id = {probe}"), Outcome::Done);
    }
    lab.expect(
        "SELECT id, v FROM t ORDER BY id",
        rows(&[&[int(5), text("a")], &[int(6), text("b")]]),
    );
}

fn redline_db() -> (tempfile::TempDir, std::sync::Arc<redlinedb_sql::Connection>) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::create(dir.path().join("db"), DbOptions::default()).unwrap();
    let conn = db.connect();
    conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)")
        .unwrap();
    conn.execute("INSERT INTO t(id, v) VALUES (5, 'a'), (6, 'b')")
        .unwrap();
    (dir, conn)
}

fn redline_rows(
    conn: &std::sync::Arc<redlinedb_sql::Connection>,
    sql: &str,
) -> Result<Vec<String>, String> {
    let mut stmt = conn.prepare(sql).map_err(|err| err.to_string())?;
    let mut out = Vec::new();
    loop {
        match stmt.step().map_err(|err| err.to_string())? {
            Step::Row => out.push(format!("{:?}", stmt.column_value(0).unwrap())),
            Step::Done => return Ok(out),
        }
    }
}

#[test]
fn rowid_equality_with_numeric_text_is_refused_not_answered_wrongly() {
    let (_dir, conn) = redline_db();
    let sqlite = rusqlite::Connection::open_in_memory().unwrap();
    sqlite
        .execute_batch(
            "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT); \
             INSERT INTO t VALUES (5, 'a'), (6, 'b');",
        )
        .unwrap();
    for probe in NUMERIC_TEXT_PROBES {
        let select = format!("SELECT v FROM t WHERE id = {probe}");
        let found: Vec<String> = sqlite
            .prepare(&select)
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(found, ["a"], "SQLite's answer to `{select}`");
        for sql in [
            select.clone(),
            format!("SELECT v FROM t WHERE {probe} = id"),
            format!("UPDATE t SET v = 'u' WHERE id = {probe}"),
            format!("DELETE FROM t WHERE id = {probe}"),
        ] {
            let err = redline_rows(&conn, &sql).expect_err(&sql);
            assert!(err.contains("comparison affinity"), "`{sql}`: {err}");
        }
    }
    assert_eq!(
        redline_rows(&conn, "SELECT v FROM t ORDER BY id").unwrap(),
        [r#"Text("a")"#, r#"Text("b")"#]
    );
}

/// A bound parameter takes the same path as a literal.
#[test]
fn rowid_equality_with_a_bound_parameter_takes_the_literal_path() {
    let (_dir, conn) = redline_db();
    let found = |bind: &dyn Fn(&mut redlinedb_sql::Statement) -> redlinedb_sql::Result<()>| {
        let mut stmt = conn.prepare("SELECT v FROM t WHERE id = ?").unwrap();
        bind(&mut stmt).unwrap();
        let mut out = Vec::new();
        loop {
            match stmt.step().map_err(|err| err.to_string())? {
                Step::Row => out.push(stmt.column_text(0).unwrap().to_owned()),
                Step::Done => return Ok::<_, String>(out),
            }
        }
    };
    assert_eq!(
        found(&|stmt| stmt.bind_f64(1, 5.0)),
        Ok(vec!["a".to_owned()])
    );
    assert_eq!(found(&|stmt| stmt.bind_f64(1, 5.5)), Ok(vec![]));
    assert_eq!(found(&|stmt| stmt.bind_i64(1, -1)), Ok(vec![]));
    assert_eq!(found(&|stmt| stmt.bind_text(1, "x")), Ok(vec![]));
    let err = found(&|stmt| stmt.bind_text(1, "5")).unwrap_err();
    assert!(err.contains("comparison affinity"), "{err}");
}

// -- foreign keys: a parent key update between INTEGER and REAL --------------

/// SQLite's ON UPDATE action runs only when `NOT(old.k IS new.k)`, and IS
/// holds between INTEGER 5 and REAL 5.0. For NO ACTION the child counts for
/// the old and the new key cancel out. So moving an untyped parent key
/// between 5 and 5.0 leaves every child alone.
fn fk_parent_update(child_decl: &str, parent_value: &str, new_value: &str) {
    let lab = Lab::new();
    lab.script(&[
        "PRAGMA foreign_keys = ON",
        "CREATE TABLE p(k UNIQUE)",
        &format!("CREATE TABLE c(id INTEGER PRIMARY KEY, pk {child_decl})"),
        &format!("INSERT INTO p(k) VALUES ({parent_value})"),
        "INSERT INTO c(pk) VALUES (5)",
    ]);
    lab.expect(
        &format!("UPDATE p SET k = {new_value} WHERE k = {parent_value}"),
        Outcome::Done,
    );
    lab.expect(
        "SELECT pk, typeof(pk) FROM c",
        rows(&[&[int(5), text("integer")]]),
    );
    lab.step("SELECT k, typeof(k) FROM p");
}

#[test]
fn fk_parent_key_update_between_integer_and_real_is_no_change() {
    for child_decl in [
        "REFERENCES p(k)",
        "REFERENCES p(k) ON UPDATE CASCADE",
        "REFERENCES p(k) ON UPDATE SET NULL",
        "REFERENCES p(k) ON UPDATE RESTRICT",
        "INTEGER REFERENCES p(k)",
        "INTEGER REFERENCES p(k) ON UPDATE CASCADE",
    ] {
        fk_parent_update(child_decl, "5", "5.0");
        fk_parent_update(child_decl, "5.0", "5");
    }
}

#[test]
fn fk_parent_key_update_to_a_different_value_still_acts() {
    let lab = Lab::new();
    lab.script(&[
        "PRAGMA foreign_keys = ON",
        "CREATE TABLE p(k UNIQUE)",
        "CREATE TABLE c(id INTEGER PRIMARY KEY, pk REFERENCES p(k))",
        "CREATE TABLE cc(id INTEGER PRIMARY KEY, pk REFERENCES p(k) ON UPDATE CASCADE)",
        "INSERT INTO p(k) VALUES (5), (7)",
        "INSERT INTO c(pk) VALUES (5)",
        "INSERT INTO cc(pk) VALUES (7)",
    ]);
    lab.expect(
        "UPDATE p SET k = 6 WHERE k = 5",
        Outcome::Err(lab::ErrKind::ForeignKey),
    );
    lab.expect("UPDATE p SET k = '7' WHERE k = 7", Outcome::Done);
    lab.expect(
        "SELECT pk, typeof(pk) FROM cc",
        rows(&[&[text("7"), text("text")]]),
    );
}

#[test]
fn fk_parent_row_replaced_with_its_key_as_real_keeps_children() {
    let lab = Lab::new();
    lab.script(&[
        "PRAGMA foreign_keys = ON",
        "CREATE TABLE p(id INTEGER PRIMARY KEY, k UNIQUE)",
        "CREATE TABLE c(id INTEGER PRIMARY KEY, pk REFERENCES p(k) ON UPDATE CASCADE)",
        "INSERT INTO p(id, k) VALUES (1, 5)",
        "INSERT INTO c(pk) VALUES (5)",
    ]);
    lab.step("INSERT OR REPLACE INTO p(id, k) VALUES (1, 5.0)");
    lab.step("SELECT pk, typeof(pk) FROM c");
    lab.step("SELECT k, typeof(k) FROM p");
}
