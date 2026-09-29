//! What the harness read back after the crash, the image a correct recovery
//! would produce, and the catalog objects the committed keys imply.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::{
    CATALOG_SLOTS, KV_INDEX, KV_TABLE, KV_TENANTS, PROGRESS_TABLE, Workload, expected_row_values,
    row_digest, scratch_index, scratch_table,
};
use crate::engine::CellValue;

/// Index read versus forced full scan for one index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct IndexCheck {
    pub index: String,
    pub table: String,
    /// EXPLAIN QUERY PLAN of the probe named `index`.
    pub index_path_used: bool,
    /// EXPLAIN QUERY PLAN of the NOT INDEXED reference read used no index.
    pub scan_avoided_index: bool,
    /// Probe value -> keys returned through the index.
    pub via_index: BTreeMap<String, BTreeSet<u64>>,
    /// Probe value -> keys returned by the forced full scan.
    pub via_scan: BTreeMap<String, BTreeSet<u64>>,
}

/// Everything the harness read back after the crash.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RecoveredState {
    /// The child printed READY after its schema setup.
    pub child_started: bool,
    /// The harness saw the fault happen (a kill that landed on a live
    /// child, or a failpoint hit marker naming the armed failpoint).
    pub fault_observed: bool,
    /// Table -> integer primary key -> row digest, for keyed tables.
    pub tables: BTreeMap<String, BTreeMap<u64, String>>,
    /// Names in sqlite_schema, excluding `sqlite_%` internals.
    pub schema: BTreeSet<String>,
    pub index_checks: Vec<IndexCheck>,
    /// Rows of `PRAGMA integrity_check`; healthy is exactly `["ok"]`.
    pub integrity_check: Vec<String>,
    /// A key a table read or an index probe returned more than once, such
    /// as the second copy a non-idempotent replay leaves. Any entry
    /// disqualifies; the maps above keep only one copy.
    pub duplicates: Vec<String>,
    /// Failures of the harness itself (open failed, ack log missing after
    /// READY, recovery passes disagree, ...). Any entry disqualifies.
    pub harness_errors: Vec<String>,
}

impl RecoveredState {
    /// The image a correct recovery of exactly `keys` would produce, with
    /// READY and the fault observed. Tests mutate it to model defects.
    pub fn consistent(
        workload: Workload,
        total_rows: usize,
        keys: impl IntoIterator<Item = u64>,
    ) -> Self {
        let keys: BTreeSet<u64> = keys.into_iter().collect();
        let mut state = Self {
            child_started: true,
            fault_observed: true,
            schema: expected_schema(workload, &keys),
            integrity_check: vec!["ok".to_owned()],
            ..Self::default()
        };
        for table in &state.schema {
            if is_keyed_table(table) {
                state.tables.insert(table.clone(), BTreeMap::new());
            }
        }
        let mut notes: BTreeMap<String, BTreeMap<String, BTreeSet<u64>>> = BTreeMap::new();
        for &key in &keys {
            for (table, values) in expected_row_values(workload, key, total_rows) {
                if let Some(CellValue::Text(note)) = values.get(1)
                    && table.starts_with("scratch_")
                {
                    notes
                        .entry(table.clone())
                        .or_default()
                        .entry(note.clone())
                        .or_default()
                        .insert(key);
                }
                state
                    .tables
                    .entry(table)
                    .or_default()
                    .insert(key, row_digest(&values));
            }
        }
        let mut tenants: BTreeMap<String, BTreeSet<u64>> = (0..KV_TENANTS)
            .map(|tenant| (tenant.to_string(), BTreeSet::new()))
            .collect();
        for key in state
            .tables
            .get(KV_TABLE)
            .into_iter()
            .flat_map(|rows| rows.keys())
        {
            tenants
                .entry((key % KV_TENANTS).to_string())
                .or_default()
                .insert(*key);
        }
        state.index_checks.push(IndexCheck {
            index: KV_INDEX.to_owned(),
            table: KV_TABLE.to_owned(),
            index_path_used: true,
            scan_avoided_index: true,
            via_index: tenants.clone(),
            via_scan: tenants,
        });
        for (table, by_note) in notes {
            let slot: u64 = table.trim_start_matches("scratch_").parse().unwrap_or(0);
            state.index_checks.push(IndexCheck {
                index: scratch_index(slot),
                table,
                index_path_used: true,
                scan_avoided_index: true,
                via_index: by_note.clone(),
                via_scan: by_note,
            });
        }
        state
    }

    /// The parts a second recovery pass must reproduce exactly.
    pub fn image_differences(&self, other: &Self) -> Vec<String> {
        let mut out = Vec::new();
        let tables: BTreeSet<&String> = self.tables.keys().chain(other.tables.keys()).collect();
        for table in tables {
            if self.tables.get(table) != other.tables.get(table) {
                out.push(format!("table {table} differs between recovery passes"));
            }
        }
        if self.schema != other.schema {
            out.push(format!(
                "schema differs between recovery passes: {:?} vs {:?}",
                self.schema, other.schema
            ));
        }
        if self.index_checks != other.index_checks {
            out.push("index checks differ between recovery passes".to_owned());
        }
        if self.duplicates != other.duplicates {
            out.push(format!(
                "duplicate keys differ between recovery passes: {:?} vs {:?}",
                self.duplicates, other.duplicates
            ));
        }
        if self.integrity_check != other.integrity_check {
            out.push(format!(
                "integrity_check differs between recovery passes: {:?} vs {:?}",
                self.integrity_check, other.integrity_check
            ));
        }
        out
    }
}

pub fn is_keyed_table(name: &str) -> bool {
    name == KV_TABLE || name == PROGRESS_TABLE || scratch_slot(name).is_some()
}

pub(super) fn scratch_slot(name: &str) -> Option<u64> {
    let slot: u64 = name.strip_prefix("scratch_")?.parse().ok()?;
    (slot < CATALOG_SLOTS).then_some(slot)
}

/// Catalog objects implied by the committed keys.
pub fn expected_schema(workload: Workload, committed: &BTreeSet<u64>) -> BTreeSet<String> {
    let mut names = workload.base_schema();
    if workload == Workload::RecoverCatalog {
        for key in committed.iter().filter(|key| *key % 2 == 1) {
            names.insert(scratch_table(key % CATALOG_SLOTS));
            names.insert(scratch_index(key % CATALOG_SLOTS));
        }
    }
    names
}
