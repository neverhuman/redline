//! A TEXT = TEXT inner equijoin reads the right table through its index.
//!
//! `index_usable(Text, Some(Text))` answered false, unlike SQLite's
//! `sqlite3IndexAffinityOk` (two non-numeric affinities compare as BLOB,
//! and a BLOB comparison can use any index), so every such join fell back
//! to pairing each left row with every right row.

use tempfile::tempdir;

use crate::connection::{Database, DbOptions};
use crate::exec::tail::{take_table_row_decodes, take_table_row_loads};
use crate::statement::Step;
use crate::value::SqlValue;

fn rows(conn: &std::sync::Arc<crate::connection::Connection>, sql: &str) -> Vec<Vec<SqlValue>> {
    let mut stmt = conn.prepare(sql).unwrap();
    let mut out = Vec::new();
    while stmt.step().unwrap() == Step::Row {
        out.push(
            (0..stmt.column_count())
                .map(|i| stmt.column_value(i).unwrap().clone())
                .collect(),
        );
    }
    out
}

#[test]
fn text_equijoin_probes_the_right_index() {
    let dir = tempdir().unwrap();
    let db = Database::create(dir.path().join("join.db"), DbOptions::default()).unwrap();
    let conn = db.connect();
    conn.execute(
        "CREATE TABLE a(email TEXT); CREATE TABLE b(email TEXT, v INTEGER); \
         CREATE INDEX b_email ON b(email);",
    )
    .unwrap();
    conn.execute("INSERT INTO a VALUES ('u7@x'), ('u42@x'), ('nobody@x')")
        .unwrap();
    let mut insert = conn.prepare("INSERT INTO b VALUES (?1, ?2)").unwrap();
    for i in 0..400 {
        insert
            .bind_text(1, std::sync::Arc::from(format!("u{i}@x").as_str()))
            .unwrap();
        insert.bind_i64(2, i).unwrap();
        assert_eq!(insert.step().unwrap(), Step::Done);
        insert.reset().unwrap();
    }
    let _ = (take_table_row_decodes(), take_table_row_loads());
    let got = rows(
        &conn,
        "SELECT a.email, b.v FROM a JOIN b ON a.email = b.email ORDER BY b.v",
    );
    let decodes = take_table_row_decodes();
    let loads = take_table_row_loads();
    assert_eq!(
        got,
        vec![
            vec![SqlValue::Text("u7@x".into()), SqlValue::Integer(7)],
            vec![SqlValue::Text("u42@x".into()), SqlValue::Integer(42)],
        ]
    );
    // Row by row the join decodes all 400 rows of `b`; the probe reads the
    // two that match.
    assert!(
        decodes < 100,
        "the join decoded {decodes} rows ({loads} loads): it did not probe b_email"
    );
}
