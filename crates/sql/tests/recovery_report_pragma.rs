//! `PRAGMA redline_recovery_report`: what the recovery that opened this
//! database found and did (workplan R9, step 4).

use std::path::Path;
use std::sync::Arc;

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use tempfile::tempdir;

/// Column name to value, for the single row the PRAGMA returns, or `None`
/// when it returns no row.
fn recovery_report(conn: &Arc<Connection>) -> Option<Vec<(String, SqlValue)>> {
    let mut stmt = conn
        .prepare("PRAGMA redline_recovery_report")
        .expect("prepare pragma");
    let mut rows = Vec::new();
    while let Step::Row = stmt.step().expect("step") {
        let row = (0..stmt.column_count())
            .map(|index| {
                (
                    stmt.column_name(index).to_owned(),
                    stmt.column_value(index).expect("column value").clone(),
                )
            })
            .collect::<Vec<_>>();
        rows.push(row);
    }
    assert!(rows.len() <= 1, "more than one report row: {rows:?}");
    rows.pop()
}

fn field<'a>(row: &'a [(String, SqlValue)], name: &str) -> &'a SqlValue {
    &row.iter()
        .find(|(column, _)| column == name)
        .unwrap_or_else(|| panic!("no column {name} in {row:?}"))
        .1
}

fn integer(row: &[(String, SqlValue)], name: &str) -> i64 {
    match field(row, name) {
        SqlValue::Integer(value) => *value,
        other => panic!("{name} is {other:?}"),
    }
}

fn text(row: &[(String, SqlValue)], name: &str) -> String {
    match field(row, name) {
        SqlValue::Text(value) => value.to_string(),
        other => panic!("{name} is {other:?}"),
    }
}

fn truncate_last_segment(path: &Path, bytes: u64) {
    let segment = path.join("wal").join(format!("{:020}.wal", 1));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(segment)
        .expect("open segment");
    let len = file.metadata().expect("metadata").len();
    file.set_len(len - bytes).expect("truncate");
}

#[test]
fn recovery_report_pragma_shows_the_torn_tail_and_its_salvage_file() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("recovery-report.db");
    let db = Database::create(&path, DbOptions::default()).expect("create database");
    let conn = db.connect();
    // A database this process created was not recovered.
    assert_eq!(recovery_report(&conn), None);
    conn.execute("CREATE TABLE t(a INTEGER PRIMARY KEY, b TEXT)")
        .expect("create table");
    conn.execute("INSERT INTO t VALUES (1, 'one')")
        .expect("insert");
    conn.execute("BEGIN").expect("begin");
    conn.execute("INSERT INTO t VALUES (2, 'two')")
        .expect("insert in flight");
    drop(conn);
    drop(db);
    // The in-flight insert's last record loses its last bytes, as a crash
    // in the middle of that write leaves it.
    truncate_last_segment(&path, 3);

    let db = Database::open(&path, DbOptions::default()).expect("reopen");
    let conn = db.connect();
    let row = recovery_report(&conn).expect("a reopened database has a report");
    assert_eq!(integer(&row, "torn_tail"), 1, "{row:?}");
    assert!(integer(&row, "torn_tail_bytes") > 0, "{row:?}");
    assert_eq!(integer(&row, "torn_tail_segment"), 1, "{row:?}");
    let salvage = text(&row, "salvage");
    assert!(
        salvage.starts_with("wal/salvage/") && salvage.ends_with(".torn"),
        "{row:?}"
    );
    assert!(path.join(&salvage).is_file(), "{salvage} is not a file");
    assert!(integer(&row, "valid_end_lsn") > 0, "{row:?}");
    assert!(integer(&row, "scanned_records") > 0, "{row:?}");
    assert_eq!(text(&row, "target"), "latest", "{row:?}");

    // The recovered tail follows the documented policy: not an integrity
    // error.
    let mut check = conn.prepare("PRAGMA integrity_check").expect("prepare");
    assert_eq!(check.step().expect("step"), Step::Row);
    assert_eq!(check.column_text(0).expect("integrity"), "ok");
    drop(check);
    drop(conn);
    drop(db);

    // The next open finds a whole log.
    let db = Database::open(&path, DbOptions::default()).expect("reopen again");
    let row = recovery_report(&db.connect()).expect("report");
    assert_eq!(integer(&row, "torn_tail"), 0, "{row:?}");
    assert_eq!(field(&row, "salvage"), &SqlValue::Null, "{row:?}");
}
