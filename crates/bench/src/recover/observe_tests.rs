//! The observer must report a key a table or an index returns twice.
//!
//! A non-idempotent replay leaves a second copy of an acknowledged row.
//! Collecting rows into a map by key, and index results into a set, kept
//! one copy and the oracle qualified the run. These cases read a SQLite
//! file whose tables have no key constraint, so a duplicate can be stored
//! the way a broken recovery would leave it.

use rusqlite::Connection;

use super as observe;
use crate::config::{DurabilityKind, EngineKind};
use crate::engine::CellValue;
use crate::recover::oracle::{
    self, AckLedger, KV_INDEX, KV_TABLE, PROGRESS_TABLE, RecoveredState, Workload,
};

const ROWS: usize = 16;

fn insert(conn: &Connection, table: &str, values: &[CellValue]) {
    let placeholders = vec!["?"; values.len()].join(", ");
    let params: Vec<rusqlite::types::Value> = values
        .iter()
        .map(|value| match value {
            CellValue::Null => rusqlite::types::Value::Null,
            CellValue::Integer(v) => rusqlite::types::Value::Integer(*v),
            CellValue::Real(v) => rusqlite::types::Value::Real(*v),
            CellValue::Text(v) => rusqlite::types::Value::Text(v.clone()),
            CellValue::Blob(v) => rusqlite::types::Value::Blob(v.clone()),
        })
        .collect();
    conn.execute(
        &format!("INSERT INTO {table} VALUES ({placeholders})"),
        rusqlite::params_from_iter(params),
    )
    .expect("insert");
}

/// A recovered `wal` workload image for keys `0..4`, plus `extra` more
/// copies of the kv row for key 2.
fn observed_with_extra_copies(extra: usize) -> RecoveredState {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = observe::database_path(EngineKind::Sqlite, dir.path());
    let conn = Connection::open(&path).expect("open sqlite");
    // The test reads what it wrote; it does not need to wait for fsync.
    conn.execute_batch("PRAGMA synchronous = OFF; BEGIN;")
        .expect("pragmas");
    conn.execute_batch(&format!(
        "CREATE TABLE {KV_TABLE}(k INTEGER, tenant INTEGER, v BLOB, version INTEGER);
         CREATE INDEX {KV_INDEX} ON {KV_TABLE}(tenant);
         CREATE TABLE {PROGRESS_TABLE}(id INTEGER, scenario TEXT, note TEXT);"
    ))
    .expect("schema");
    for key in 0..4 {
        insert(
            &conn,
            KV_TABLE,
            &oracle::kv_row_values(Workload::RecoverWal, key, ROWS),
        );
        insert(
            &conn,
            PROGRESS_TABLE,
            &oracle::progress_row_values(Workload::RecoverWal, key),
        );
    }
    for _ in 0..extra {
        insert(
            &conn,
            KV_TABLE,
            &oracle::kv_row_values(Workload::RecoverWal, 2, ROWS),
        );
    }
    conn.execute_batch("COMMIT;").expect("commit");
    drop(conn);
    let mut state = observe::observe(
        EngineKind::Sqlite,
        DurabilityKind::Normal,
        dir.path(),
        false,
    )
    .expect("observe");
    state.child_started = true;
    state.fault_observed = true;
    state
}

fn verdict(state: &RecoveredState) -> oracle::RecoveryVerdict {
    let mut ledger = AckLedger::new(Workload::RecoverWal, ROWS, true);
    for key in 0..4 {
        ledger.ack(key);
    }
    oracle::evaluate(&ledger, state)
}

#[test]
fn an_image_with_each_row_once_qualifies() {
    let state = observed_with_extra_copies(0);
    let verdict = verdict(&state);
    assert!(verdict.qualified, "{}", verdict.summary());
}

#[test]
fn a_second_copy_of_an_acknowledged_row_disqualifies() {
    let state = observed_with_extra_copies(1);
    assert!(
        state
            .duplicates
            .iter()
            .any(|line| line.contains(KV_TABLE) && line.contains("key 2")),
        "{:?}",
        state.duplicates
    );
    assert!(
        state.duplicates.iter().any(|line| line.contains(KV_INDEX)),
        "the index probe's second copy was not reported: {:?}",
        state.duplicates
    );
    let verdict = verdict(&state);
    assert!(
        !verdict.qualified,
        "a duplicated acknowledged row qualified"
    );
    assert!(
        verdict
            .integrity_errors
            .iter()
            .any(|line| line.contains("key 2")),
        "{:?}",
        verdict.integrity_errors
    );
}
