//! `PRAGMA integrity_check`: every index's entries against its table.
//!
//! The kernel checks the pages, the WAL and each B-tree's structure. It
//! cannot evaluate SQL, so it cannot say which rows an expression or partial
//! index should hold. This check reads each table and its indexes at one
//! snapshot, derives every index's expected `(key, rowid)` entries from the
//! rows with the key builder and partial-index predicate that DML
//! maintenance uses, and compares them with the entries the index holds.
//!
//! It reports what SQLite reports -- `row N missing from index I`,
//! `non-unique entry in UNIQUE index I`, `wrong # of entries in index I` --
//! and also names an entry the index holds that no row produces (an entry a
//! row kept after it left a partial index, for example).

use std::collections::{HashMap, HashSet};
use std::ops::Bound;
use std::sync::Arc;

use redlinedb_kernel::catalog::{IndexDef, TableDef};
use redlinedb_kernel::engine::Txn;
use redlinedb_kernel::format::RowId;
use redlinedb_kernel::index::{BtreeIndex, CursorYield, IndexCursor, KeyRange, SnapshotView};
use redlinedb_kernel::txn::Isolation;

use crate::connection::Connection;
use crate::error::Result;

use super::expr::scalar::row::TableRow;
use super::{collect_table_rows, index_dml, index_predicate};

/// SQLite stops after this many errors by default.
const MAX_ERRORS: usize = 100;

/// Entries read from an index per cursor batch.
const BATCH: usize = 1024;

/// The rows of `PRAGMA integrity_check` before `ok` is substituted for an
/// empty list: the kernel's findings, then the index-content findings.
pub(crate) fn integrity_check(conn: &Connection) -> Result<Vec<String>> {
    let mut errors = conn.integrity_check()?;
    if errors.len() < MAX_ERRORS {
        errors.extend(index_content_errors(conn, MAX_ERRORS - errors.len())?);
    }
    Ok(errors)
}

/// Compare every physical index with its table at one read snapshot.
fn index_content_errors(conn: &Connection, limit: usize) -> Result<Vec<String>> {
    let engine = conn.engine();
    let mut tx = engine.begin(Isolation::Snapshot)?;
    let result = check_tables(conn, &mut tx, limit);
    engine.rollback(tx)?;
    result
}

fn check_tables(conn: &Connection, tx: &mut Txn, limit: usize) -> Result<Vec<String>> {
    let engine = conn.engine();
    let snapshot = engine.schema_snapshot_for_tx(tx);
    let mut errors = Vec::new();
    for table in &snapshot.tables {
        let indexes: Vec<(&IndexDef, Arc<BtreeIndex>)> = table
            .indexes
            .iter()
            .filter_map(|index| {
                index_dml::open_index_handle_for_tx(engine, tx, index).map(|handle| (index, handle))
            })
            .collect();
        if indexes.is_empty() {
            continue;
        }
        let rows = collect_table_rows(engine, tx, table)?;
        for (index, handle) in indexes {
            check_index(conn, tx, table, index, &handle, &rows, &mut errors)?;
            if errors.len() >= limit {
                errors.truncate(limit);
                return Ok(errors);
            }
        }
    }
    Ok(errors)
}

fn check_index(
    conn: &Connection,
    tx: &Txn,
    table: &TableDef,
    index: &IndexDef,
    handle: &BtreeIndex,
    rows: &[TableRow],
    errors: &mut Vec<String>,
) -> Result<()> {
    // What the rows say the index holds: a multiset of (key, rowid).
    let mut expected: HashMap<(Vec<u8>, RowId), usize> = HashMap::new();
    let mut expected_count = 0_usize;
    let mut unique_keys: HashSet<Vec<u8>> = HashSet::new();
    let mut duplicate_key = false;
    for row in rows {
        if let Some(pred_sql) = index.predicate_sql.as_deref()
            && !index_predicate::eval_index_predicate(table, pred_sql, &row.values)?
        {
            continue;
        }
        let key = index_dml::build_index_key(table, index, &row.values)?;
        if index.unique && !key.contains_null && !unique_keys.insert(key.bytes.clone()) {
            duplicate_key = true;
        }
        *expected.entry((key.bytes, row.rowid)).or_default() += 1;
        expected_count += 1;
    }

    // What the index holds at the same snapshot.
    let engine = conn.engine();
    let view = SnapshotView::visible(engine.tx_status(), tx.snapshot(), Some(tx.id()));
    let range = KeyRange {
        start: Bound::Unbounded,
        end: Bound::Unbounded,
    };
    let mut cursor = IndexCursor::open(handle, range, view)?;
    let mut entries = Vec::new();
    while let CursorYield::Batch(_) = cursor.next_batch_with_keys(&mut entries, BATCH)? {}
    let actual_count = entries.len();
    let mut stray: Vec<RowId> = Vec::new();
    for (key, row) in entries {
        match expected.get_mut(&(key, row.row_id)) {
            Some(left) if *left > 0 => *left -= 1,
            _ => stray.push(row.row_id),
        }
    }
    let mut missing: Vec<RowId> = expected
        .into_iter()
        .filter(|(_, left)| *left > 0)
        .map(|((_, rowid), _)| rowid)
        .collect();
    missing.sort_unstable();
    stray.sort_unstable();

    let name = &index.name;
    for rowid in missing {
        errors.push(format!("row {} missing from index {name}", rowid.0));
    }
    let live: HashSet<RowId> = rows.iter().map(|row| row.rowid).collect();
    for rowid in stray {
        errors.push(if live.contains(&rowid) {
            format!(
                "index {name} has an entry for row {} that the row does not produce",
                rowid.0
            )
        } else {
            format!(
                "index {name} has an entry for row {}, which is not in table {}",
                rowid.0, table.name
            )
        });
    }
    if duplicate_key {
        errors.push(format!("non-unique entry in UNIQUE index {name}"));
    }
    if actual_count != expected_count {
        errors.push(format!("wrong # of entries in index {name}"));
    }
    Ok(())
}
