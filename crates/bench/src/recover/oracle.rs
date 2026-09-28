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

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::RecoveryScenarioKind;
use crate::engine::CellValue;

pub const KV_TABLE: &str = "kv";
pub const KV_INDEX: &str = "kv_tenant_idx";
pub const PROGRESS_TABLE: &str = "crash_progress";
/// kv rows use `tenant = key % KV_TENANTS`.
pub const KV_TENANTS: u64 = 32;
/// The catalog scenario writes into `scratch_{key % CATALOG_SLOTS}`.
pub const CATALOG_SLOTS: u64 = 8;
/// At most this many descriptive lines are kept per verdict list.
const MAX_DETAIL_LINES: usize = 64;

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

/// What the child acknowledged, plus what the run expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AckLedger {
    pub workload: Workload,
    pub total_rows: usize,
    /// Acknowledged key -> transaction digest the child recorded.
    pub acked: BTreeMap<u64, String>,
    /// The run is only evidence when the harness observed the fault.
    pub expect_fault: bool,
    /// Bytes after the last newline: an ack the child never finished
    /// writing, so that key counts as in flight, not acknowledged.
    pub torn_tail: Option<String>,
}

impl AckLedger {
    pub fn new(workload: Workload, total_rows: usize, expect_fault: bool) -> Self {
        Self {
            workload,
            total_rows,
            acked: BTreeMap::new(),
            expect_fault,
            torn_tail: None,
        }
    }

    /// Record `key` as acknowledged with its expected digest.
    pub fn ack(&mut self, key: u64) {
        let digest = expected_txn_digest(self.workload, key, self.total_rows);
        self.acked.insert(key, digest);
    }

    /// Parse a ledger. Fails closed on any complete line that is malformed,
    /// out of order, or whose digest differs from the workload's contents
    /// for that key (for example a child built from other sources).
    pub fn parse(
        workload: Workload,
        total_rows: usize,
        expect_fault: bool,
        text: &str,
    ) -> Result<Self> {
        let mut ledger = Self::new(workload, total_rows, expect_fault);
        let (complete, tail) = match text.rfind('\n') {
            Some(end) => (Some(&text[..end]), &text[end + 1..]),
            None => (None, text),
        };
        if !tail.is_empty() {
            ledger.torn_tail = Some(tail.to_owned());
        }
        for (line_no, line) in complete.into_iter().flat_map(|c| c.split('\n')).enumerate() {
            let line_no = line_no + 1;
            let Some((key, digest)) = line.split_once('\t') else {
                bail!("ack ledger line {line_no}: expected `key<TAB>digest`, got {line:?}");
            };
            let key: u64 = key
                .parse()
                .with_context(|| format!("ack ledger line {line_no}: key {key:?}"))?;
            if key != line_no as u64 - 1 {
                bail!(
                    "ack ledger line {line_no}: key {key} out of order (children ack keys 0,1,2,... in order)"
                );
            }
            let expected = expected_txn_digest(workload, key, total_rows);
            if digest != expected {
                bail!(
                    "ack ledger line {line_no}: digest {digest} for key {key} does not match the workload contents {expected}"
                );
            }
            ledger.acked.insert(key, digest.to_owned());
        }
        Ok(ledger)
    }

    /// The one transaction that may have committed without an ack.
    pub fn in_flight_key(&self) -> Option<u64> {
        let next = self.acked.keys().next_back().map_or(0, |last| last + 1);
        (next < self.total_rows as u64).then_some(next)
    }
}

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

fn scratch_slot(name: &str) -> Option<u64> {
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

/// The oracle's decision for one run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryVerdict {
    pub acknowledged: usize,
    /// Acknowledged transactions recovered with their exact contents.
    pub recovered_acked: usize,
    /// The unacknowledged in-flight key, when it did commit.
    pub in_flight_committed: Option<u64>,
    pub lost_ack_ids: Vec<u64>,
    pub unexpected_effects: Vec<String>,
    pub partial_transactions: Vec<u64>,
    pub integrity_errors: Vec<String>,
    pub harness_errors: Vec<String>,
    pub fault_observed: bool,
    pub fault_required: bool,
    pub child_started: bool,
    pub qualified: bool,
}

impl RecoveryVerdict {
    /// One line naming every reason the run is not qualified.
    pub fn summary(&self) -> String {
        if self.qualified {
            return format!(
                "qualified: {} acked transactions recovered exactly{}",
                self.recovered_acked,
                match self.in_flight_committed {
                    Some(key) => format!(", in-flight key {key} committed atomically"),
                    None => String::new(),
                }
            );
        }
        let mut reasons = Vec::new();
        if !self.child_started {
            reasons.push("child never reported READY".to_owned());
        }
        if self.fault_required && !self.fault_observed {
            reasons.push("no fault was observed".to_owned());
        }
        if !self.lost_ack_ids.is_empty() {
            reasons.push(format!(
                "{} acked transactions lost or changed (first: {:?})",
                self.lost_ack_ids.len(),
                &self.lost_ack_ids[..self.lost_ack_ids.len().min(8)]
            ));
        }
        if !self.partial_transactions.is_empty() {
            reasons.push(format!(
                "partial transactions {:?}",
                &self.partial_transactions[..self.partial_transactions.len().min(8)]
            ));
        }
        for list in [
            &self.unexpected_effects,
            &self.integrity_errors,
            &self.harness_errors,
        ] {
            if let Some(first) = list.first() {
                reasons.push(format!("{first} ({} total)", list.len()));
            }
        }
        format!("not qualified: {}", reasons.join("; "))
    }
}

fn push_capped(list: &mut Vec<String>, overflow: &mut usize, line: String) {
    if list.len() < MAX_DETAIL_LINES {
        list.push(line);
    } else {
        *overflow += 1;
    }
}

/// Compare what was acknowledged with what recovery produced.
pub fn evaluate(expected: &AckLedger, observed: &RecoveredState) -> RecoveryVerdict {
    let workload = expected.workload;
    let in_flight = expected.in_flight_key();
    let mut verdict = RecoveryVerdict {
        acknowledged: expected.acked.len(),
        fault_observed: observed.fault_observed,
        fault_required: expected.expect_fault,
        child_started: observed.child_started,
        harness_errors: observed.harness_errors.clone(),
        ..RecoveryVerdict::default()
    };
    let mut unexpected_overflow = 0_usize;
    let mut integrity_overflow = 0_usize;

    let mut candidates: BTreeSet<u64> = expected.acked.keys().copied().collect();
    candidates.extend(in_flight);
    for rows in observed.tables.values() {
        candidates.extend(rows.keys().copied());
    }

    let mut committed = BTreeSet::new();
    for key in candidates {
        let present: BTreeMap<String, String> = observed
            .tables
            .iter()
            .filter_map(|(table, rows)| rows.get(&key).map(|d| (table.clone(), d.clone())))
            .collect();
        let acked_digest = expected.acked.get(&key);
        if acked_digest.is_none() && Some(key) != in_flight {
            if !present.is_empty() {
                let tables: Vec<&String> = present.keys().collect();
                push_capped(
                    &mut verdict.unexpected_effects,
                    &mut unexpected_overflow,
                    format!(
                        "key {key} has rows in {tables:?} but was never acknowledged and is not the in-flight key {in_flight:?}"
                    ),
                );
            }
            continue;
        }
        let want = expected_rows(workload, key, expected.total_rows);
        for table in present.keys().filter(|table| !want.contains_key(*table)) {
            push_capped(
                &mut verdict.unexpected_effects,
                &mut unexpected_overflow,
                format!("key {key} has a row in {table}, which its transaction never writes"),
            );
        }
        let got: BTreeMap<String, String> = want
            .keys()
            .filter_map(|table| present.get(table).map(|d| (table.clone(), d.clone())))
            .collect();
        let mut lost = false;
        if got.is_empty() {
            lost = acked_digest.is_some();
        } else {
            committed.insert(key);
            if got.len() < want.len() {
                verdict.partial_transactions.push(key);
                lost = true;
            }
            let reference = acked_digest.cloned().unwrap_or_else(|| txn_digest(&want));
            if got.len() == want.len() && txn_digest(&got) != reference {
                lost = true;
                for (table, digest) in &got {
                    if want.get(table) != Some(digest) {
                        push_capped(
                            &mut verdict.unexpected_effects,
                            &mut unexpected_overflow,
                            format!(
                                "key {key}: {table} row differs from the acknowledged contents"
                            ),
                        );
                    }
                }
                if got == want {
                    push_capped(
                        &mut verdict.unexpected_effects,
                        &mut unexpected_overflow,
                        format!("key {key}: recovered rows differ from the ledger digest"),
                    );
                }
            }
        }
        match acked_digest {
            Some(_) if lost => verdict.lost_ack_ids.push(key),
            Some(_) => verdict.recovered_acked += 1,
            None if !got.is_empty() && !lost => verdict.in_flight_committed = Some(key),
            None => {}
        }
    }

    let want_schema = expected_schema(workload, &committed);
    for name in observed.schema.difference(&want_schema) {
        push_capped(
            &mut verdict.unexpected_effects,
            &mut unexpected_overflow,
            format!("schema object {name} exists but no committed transaction leaves it"),
        );
    }
    for name in want_schema.difference(&observed.schema) {
        push_capped(
            &mut verdict.integrity_errors,
            &mut integrity_overflow,
            format!("schema object {name} is missing although committed transactions require it"),
        );
    }

    for duplicate in &observed.duplicates {
        push_capped(
            &mut verdict.integrity_errors,
            &mut integrity_overflow,
            format!("returned more than once: {duplicate}"),
        );
    }

    if observed.integrity_check != ["ok"] {
        push_capped(
            &mut verdict.integrity_errors,
            &mut integrity_overflow,
            format!(
                "PRAGMA integrity_check returned {} lines, first {:?}",
                observed.integrity_check.len(),
                &observed.integrity_check[..observed.integrity_check.len().min(4)]
            ),
        );
    }

    for index in observed
        .schema
        .iter()
        .filter(|name| is_workload_index(name))
    {
        let Some(check) = observed.index_checks.iter().find(|c| &c.index == index) else {
            push_capped(
                &mut verdict.integrity_errors,
                &mut integrity_overflow,
                format!("index {index} was not checked against a full scan"),
            );
            continue;
        };
        let table_rows = observed.tables.get(&check.table).map_or(0, BTreeMap::len);
        if table_rows > 0 && check.via_scan.is_empty() {
            push_capped(
                &mut verdict.integrity_errors,
                &mut integrity_overflow,
                format!(
                    "index {index}: no probes although {} has {table_rows} rows",
                    check.table
                ),
            );
        }
        if !check.index_path_used {
            push_capped(
                &mut verdict.integrity_errors,
                &mut integrity_overflow,
                format!("index {index}: the probe query did not read through the index"),
            );
        }
        if !check.scan_avoided_index {
            push_capped(
                &mut verdict.integrity_errors,
                &mut integrity_overflow,
                format!("index {index}: the NOT INDEXED reference read used an index"),
            );
        }
        let probes: BTreeSet<&String> = check
            .via_index
            .keys()
            .chain(check.via_scan.keys())
            .collect();
        for probe in probes {
            let empty = BTreeSet::new();
            let by_index = check.via_index.get(probe).unwrap_or(&empty);
            let by_scan = check.via_scan.get(probe).unwrap_or(&empty);
            if by_index != by_scan {
                let missing: Vec<&u64> = by_scan.difference(by_index).take(8).collect();
                let dangling: Vec<&u64> = by_index.difference(by_scan).take(8).collect();
                push_capped(
                    &mut verdict.integrity_errors,
                    &mut integrity_overflow,
                    format!(
                        "index {index} probe {probe:?}: index returned {} keys, full scan {}; missing from index {missing:?}, only in index {dangling:?}",
                        by_index.len(),
                        by_scan.len()
                    ),
                );
            }
        }
    }

    if unexpected_overflow > 0 {
        verdict
            .unexpected_effects
            .push(format!("... and {unexpected_overflow} more"));
    }
    if integrity_overflow > 0 {
        verdict
            .integrity_errors
            .push(format!("... and {integrity_overflow} more"));
    }

    verdict.qualified = verdict.child_started
        && (verdict.fault_observed || !verdict.fault_required)
        && verdict.lost_ack_ids.is_empty()
        && verdict.unexpected_effects.is_empty()
        && verdict.partial_transactions.is_empty()
        && verdict.integrity_errors.is_empty()
        && verdict.harness_errors.is_empty();
    verdict
}

fn is_workload_index(name: &str) -> bool {
    name == KV_INDEX
        || name
            .strip_suffix("_note_idx")
            .is_some_and(|table| scratch_slot(table).is_some())
}
