//! Recovery oracle: decides whether a crashed-and-recovered database holds
//! exactly the transactions the child acknowledged.
//!
//! The child appends one ledger line per acknowledged transaction,
//! `key<TAB>sha256(expected row values)`, only after the commit returned.
//! After the crash the harness reads every keyed row back into a
//! [`RecoveredState`] and [`evaluate`] compares the two sides:
//!
//! - every acknowledged key is present in every table its transaction wrote,
//!   with the exact acknowledged contents (not just a row count);
//! - the only unacknowledged transaction that may appear is the single
//!   in-flight one (the key after the last ack), and it must be complete;
//! - a transaction is never half present (atomicity across its tables);
//! - index reads agree with a forced full scan, `PRAGMA integrity_check` is
//!   `ok`, and the catalog holds exactly the objects the committed keys
//!   imply;
//! - the child reached READY (schema set up) and, when the run expects a
//!   fault, the harness saw it happen.
//!
//! A run that misses any of these is not qualified. The oracle has no
//! "count only" or "missing means zero" fallback on purpose.

#[path = "oracle/evaluate.rs"]
mod evaluate;
#[path = "oracle/ledger.rs"]
mod ledger;
#[path = "oracle/state.rs"]
mod state;
#[path = "oracle/verdict.rs"]
mod verdict;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::RecoveryScenarioKind;
use crate::engine::CellValue;

pub use evaluate::evaluate;
pub use ledger::AckLedger;
pub use state::{IndexCheck, RecoveredState, expected_schema, is_keyed_table};
pub use verdict::RecoveryVerdict;

pub const KV_TABLE: &str = "kv";
pub const KV_INDEX: &str = "kv_tenant_idx";
pub const PROGRESS_TABLE: &str = "crash_progress";
/// kv rows use `tenant = key % KV_TENANTS`.
pub const KV_TENANTS: u64 = 32;
/// The catalog scenario writes into `scratch_{key % CATALOG_SLOTS}`.
pub const CATALOG_SLOTS: u64 = 8;

/// The deterministic workload a child ran. Each key is one transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Workload {
    /// recover-child `wal`: one crash_progress row and one kv row per key.
    RecoverWal,
    /// recover-child `checkpoint`: like `wal` with other values, plus
    /// periodic checkpoints.
    RecoverCheckpoint,
    /// recover-child `catalog`: one crash_progress row per key; odd keys
    /// also leave a `scratch_{key % 8}` row, even keys create and drop
    /// their table inside the same transaction.
    RecoverCatalog,
    /// failpoint-child: one kv row per key.
    FailpointKv,
}

impl Workload {
    pub fn from_scenario(scenario: RecoveryScenarioKind) -> Self {
        match scenario {
            RecoveryScenarioKind::Wal => Self::RecoverWal,
            RecoveryScenarioKind::Checkpoint => Self::RecoverCheckpoint,
            RecoveryScenarioKind::Catalog => Self::RecoverCatalog,
        }
    }

    fn scenario_label(self) -> &'static str {
        match self {
            Self::RecoverWal => RecoveryScenarioKind::Wal.as_str(),
            Self::RecoverCheckpoint => RecoveryScenarioKind::Checkpoint.as_str(),
            Self::RecoverCatalog => RecoveryScenarioKind::Catalog.as_str(),
            Self::FailpointKv => "failpoint",
        }
    }

    fn has_progress_table(self) -> bool {
        !matches!(self, Self::FailpointKv)
    }

    /// Schema objects that exist once the child reached READY.
    pub fn base_schema(self) -> BTreeSet<String> {
        let mut names = BTreeSet::from([KV_TABLE.to_owned(), KV_INDEX.to_owned()]);
        if self.has_progress_table() {
            names.insert(PROGRESS_TABLE.to_owned());
        }
        names
    }
}

pub fn scratch_table(slot: u64) -> String {
    format!("scratch_{slot}")
}

pub fn scratch_index(slot: u64) -> String {
    format!("scratch_{slot}_note_idx")
}

/// `(k, tenant, v, version)` of the kv row written for `key`.
pub(crate) fn kv_row_values(workload: Workload, key: u64, total_rows: usize) -> Vec<CellValue> {
    let (payload, version) = match workload {
        Workload::RecoverCheckpoint => (
            format!("checkpoint-{key:08}"),
            (key % (total_rows.max(1) as u64)) as i64 + 1,
        ),
        _ => (format!("value-{key:08}"), 1),
    };
    vec![
        CellValue::Integer(key as i64),
        CellValue::Integer((key % KV_TENANTS) as i64),
        CellValue::Blob(payload.into_bytes()),
        CellValue::Integer(version),
    ]
}

/// `(id, scenario, note)` of the crash_progress row written for `key`.
pub(crate) fn progress_row_values(workload: Workload, key: u64) -> Vec<CellValue> {
    vec![
        CellValue::Integer(key as i64),
        CellValue::Text(workload.scenario_label().to_owned()),
        CellValue::Text(format!("ack-{key}")),
    ]
}

/// `(table, [id, note])` the catalog scenario inserts for `key`.
pub(crate) fn catalog_scratch_values(key: u64) -> (String, Vec<CellValue>) {
    (
        scratch_table(key % CATALOG_SLOTS),
        vec![
            CellValue::Integer(key as i64),
            CellValue::Text(format!("catalog-{key}")),
        ],
    )
}

/// Every row that transaction `key` leaves behind once committed, by table.
pub(crate) fn expected_row_values(
    workload: Workload,
    key: u64,
    total_rows: usize,
) -> BTreeMap<String, Vec<CellValue>> {
    let mut rows = BTreeMap::new();
    if workload.has_progress_table() {
        rows.insert(
            PROGRESS_TABLE.to_owned(),
            progress_row_values(workload, key),
        );
    }
    match workload {
        Workload::RecoverWal | Workload::RecoverCheckpoint | Workload::FailpointKv => {
            rows.insert(
                KV_TABLE.to_owned(),
                kv_row_values(workload, key, total_rows),
            );
        }
        Workload::RecoverCatalog => {
            if key % 2 == 1 {
                let (table, values) = catalog_scratch_values(key);
                rows.insert(table, values);
            }
        }
    }
    rows
}

/// Type-tagged SHA-256 of one row's column values.
pub(crate) fn row_digest(values: &[CellValue]) -> String {
    let mut hasher = Sha256::new();
    for value in values {
        crate::engine::hash_cell(&mut hasher, value);
    }
    format!("{:x}", hasher.finalize())
}

/// Digest of a whole transaction from its per-table row digests.
pub fn txn_digest(rows: &BTreeMap<String, String>) -> String {
    let mut hasher = Sha256::new();
    for (table, digest) in rows {
        hasher.update(table.as_bytes());
        hasher.update([0]);
        hasher.update(digest.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

/// Per-table row digests that transaction `key` must leave behind.
pub fn expected_rows(workload: Workload, key: u64, total_rows: usize) -> BTreeMap<String, String> {
    expected_row_values(workload, key, total_rows)
        .into_iter()
        .map(|(table, values)| (table, row_digest(&values)))
        .collect()
}

pub fn expected_txn_digest(workload: Workload, key: u64, total_rows: usize) -> String {
    txn_digest(&expected_rows(workload, key, total_rows))
}

/// One ledger line, newline included. Children write it with a single
/// `write_all` so a kill cannot leave a key without its digest.
pub fn ack_line(key: u64, digest: &str) -> String {
    format!("{key}\t{digest}\n")
}
