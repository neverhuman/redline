//! Keep the row loaded for an index visibility recheck when the SELECT
//! will need that same row for projection.

use std::sync::Arc;

use redlinedb_kernel::catalog::TableDef;
use redlinedb_kernel::engine::{Engine, Txn};
use redlinedb_kernel::format::RowId;

use crate::error::Result;
use crate::value::SqlValue;

use super::expr::scalar::row::TableRow;
use super::tail::load_table_row_by_rowid;

pub(super) trait RecheckSink {
    fn with_capacity(capacity: usize, max_loaded_bytes: usize) -> Self;
    fn len(&self) -> usize;
    fn push_row(&mut self, row: TableRow);
}

impl RecheckSink for Vec<RowId> {
    fn with_capacity(capacity: usize, _max_loaded_bytes: usize) -> Self {
        Vec::with_capacity(capacity)
    }

    fn len(&self) -> usize {
        Vec::len(self)
    }

    fn push_row(&mut self, row: TableRow) {
        self.push(row.rowid);
    }
}

/// A small result keeps its decoded rows; a broad probe keeps only rowids,
/// as the old SELECT runtime did. This conversion happens during the probe,
/// so even a LIMIT on a broad range never retains its entire row payload.
pub(super) enum SelectProbeRows {
    Loaded {
        rows: Vec<TableRow>,
        bytes: usize,
        max_bytes: usize,
    },
    RowIds(Vec<RowId>),
}

impl RecheckSink for SelectProbeRows {
    fn with_capacity(capacity: usize, max_loaded_bytes: usize) -> Self {
        Self::Loaded {
            rows: Vec::with_capacity(capacity.min(256)),
            bytes: 0,
            max_bytes: max_loaded_bytes.min(1024 * 1024),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Loaded { rows, .. } => rows.len(),
            Self::RowIds(rowids) => rowids.len(),
        }
    }

    fn push_row(&mut self, row: TableRow) {
        match self {
            Self::Loaded {
                rows,
                bytes,
                max_bytes,
            } => {
                let next_bytes = bytes.saturating_add(row_bytes(&row));
                if rows.len() < 256 && next_bytes <= *max_bytes {
                    rows.push(row);
                    *bytes = next_bytes;
                } else {
                    let mut rowids = Vec::with_capacity(rows.len() + 1);
                    rowids.extend(rows.drain(..).map(|loaded| loaded.rowid));
                    rowids.push(row.rowid);
                    *self = Self::RowIds(rowids);
                }
            }
            Self::RowIds(rowids) => rowids.push(row.rowid),
        }
    }
}

fn row_bytes(row: &TableRow) -> usize {
    std::mem::size_of::<TableRow>()
        .saturating_add(
            row.values
                .len()
                .saturating_mul(std::mem::size_of::<SqlValue>()),
        )
        .saturating_add(row.values.iter().fold(0usize, |bytes, value| {
            bytes.saturating_add(match value {
                SqlValue::Text(text) => text.len(),
                SqlValue::Blob(blob) => blob.len(),
                _ => 0,
            })
        }))
}

pub(super) fn load_visible_row(
    engine: &Engine,
    tx: &mut Txn,
    table: &Arc<TableDef>,
    rowid: RowId,
) -> Result<Option<TableRow>> {
    load_table_row_by_rowid(engine, tx, table, rowid)
}
