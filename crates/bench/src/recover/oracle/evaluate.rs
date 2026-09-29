//! [`evaluate`]: the comparison of what the child acknowledged with what
//! recovery produced, key by key, then the catalog, the integrity check and
//! every workload index against a forced full scan.

use std::collections::{BTreeMap, BTreeSet};

use super::state::scratch_slot;
use super::{
    AckLedger, KV_INDEX, RecoveredState, RecoveryVerdict, expected_rows, expected_schema,
    txn_digest,
};

/// At most this many descriptive lines are kept per verdict list.
const MAX_DETAIL_LINES: usize = 64;

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
        // The forced scan must cover exactly the rows the table read
        // returned; probes pre-seeded with no keys would otherwise pass an
        // index that matches nothing in the table.
        let scanned: BTreeSet<u64> = check.via_scan.values().flatten().copied().collect();
        let table_keys: BTreeSet<u64> = observed
            .tables
            .get(&check.table)
            .map(|rows| rows.keys().copied().collect())
            .unwrap_or_default();
        if scanned != table_keys {
            let missing: Vec<&u64> = table_keys.difference(&scanned).take(8).collect();
            let extra: Vec<&u64> = scanned.difference(&table_keys).take(8).collect();
            push_capped(
                &mut verdict.integrity_errors,
                &mut integrity_overflow,
                format!(
                    "index {index}: the full scan of {} returned {} keys, the table read {}; missing from the scan {missing:?}, only in the scan {extra:?}",
                    check.table,
                    scanned.len(),
                    table_keys.len()
                ),
            );
        }
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
