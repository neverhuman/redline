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
