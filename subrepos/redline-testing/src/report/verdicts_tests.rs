use std::collections::BTreeSet;

use super::{Verdicts, case_verdicts, reduce_sqlite_verdicts};
use crate::report::types::RawRecord;

fn record(case_id: &str, role: &str, status: &str) -> RawRecord {
    let repetition = role
        .strip_prefix("measured:")
        .and_then(|repetition| repetition.parse::<usize>().ok());
    serde_json::from_value(serde_json::json!({
        "case_id": case_id,
        "name": "N",
        "priority": "P0",
        "profile": "memory",
        "category": "C",
        "sample_role": role,
        "repetition_index": repetition,
        "status": status,
        "reference_elapsed_ns": 1u64,
        "target_elapsed_ns": 1u64,
    }))
    .expect("raw record")
}

/// A case's warmups and measured repetitions, all with `status`.
fn case(case_id: &str, warmup: usize, repetitions: usize, status: &str) -> Vec<RawRecord> {
    let mut records = (0..warmup)
        .map(|_| record(case_id, "warmup", status))
        .collect::<Vec<_>>();
    records.extend((1..=repetitions).map(|k| record(case_id, &format!("measured:{k}"), status)));
    records
}

fn ids(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

fn reduce_error(
    records: &[RawRecord],
    manifest: &[&str],
    warmup: usize,
    repetitions: usize,
) -> String {
    match reduce_sqlite_verdicts(records, &ids(manifest), warmup, repetitions) {
        Ok(verdicts) => panic!("reduced inconsistent samples: {verdicts:?}"),
        Err(error) => format!("{error:#}"),
    }
}

#[test]
fn verdict_rejects_duplicate_samples() {
    let mut records = case("A", 0, 3, "passed");
    records.push(record("A", "measured:1", "passed"));
    let error = reduce_error(&records, &["A"], 0, 3);
    assert!(
        error.contains("case A: duplicate sample measured:1"),
        "{error}"
    );
    // Two warmups are one duplicated warmup unless their sample index
    // tells them apart.
    let error = reduce_error(&case("A", 2, 1, "passed"), &["A"], 2, 1);
    assert!(error.contains("duplicate sample warmup"), "{error}");
    let mut indexed = case("A", 0, 1, "passed");
    for index in 0..2 {
        let mut warmup = record("A", "warmup", "passed");
        warmup.sample_index = Some(index);
        indexed.push(warmup);
    }
    indexed[0].sample_index = Some(2);
    reduce_sqlite_verdicts(&indexed, &ids(&["A"]), 2, 1).expect("distinct warmups");
    // A duplicated placeholder is a duplicate too.
    let skip = || record("S", "skipped", "skipped");
    assert!(reduce_error(&[skip(), skip()], &["S"], 0, 1).contains("duplicate sample placeholder"));
}

#[test]
fn verdict_rejects_per_case_incomplete_reps() {
    // A has repetition 1 only; B has all three, so the run as a whole
    // shows every repetition index.
    let mut records = case("A", 0, 1, "passed");
    records.extend(case("B", 0, 3, "passed"));
    let error = reduce_error(&records, &["A", "B"], 0, 3);
    assert!(
        error.contains("case A: expected measured repetitions 1..=3, found [1]"),
        "{error}"
    );
    // Ported from the warmup validation: missing warmups are per case,
    // declared skips have none, and a missing warmup cannot be balanced by
    // another case's extra one.
    let mut records = case("executed", 1, 1, "passed");
    records.push(record("skipped", "skipped", "skipped"));
    reduce_sqlite_verdicts(&records, &ids(&["executed", "skipped"]), 1, 1)
        .expect("a declared skip has no samples");
    let mut records = case("executed", 0, 1, "passed");
    records.push(record("skipped", "skipped", "skipped"));
    let error = reduce_error(&records, &["executed", "skipped"], 1, 1);
    assert!(
        error.contains("case executed: expected 1 warmup samples but found 0"),
        "{error}"
    );
    let mut records = case("missing", 0, 1, "passed");
    records.extend(case("extra", 2, 1, "passed"));
    assert!(reduce_sqlite_verdicts(&records, &ids(&["missing", "extra"]), 1, 1).is_err());
    // A repetition past the plan, or one whose index disagrees with its role.
    let mut records = case("A", 0, 3, "passed");
    records.push(record("A", "measured:4", "passed"));
    assert!(reduce_error(&records, &["A"], 0, 3).contains("outside repetitions 1..=3"));
    let mut records = case("A", 0, 2, "passed");
    records[1].repetition_index = Some(1);
    assert!(reduce_error(&records, &["A"], 0, 2).contains("carries repetition_index Some(1)"));
    // A placeholder beside samples is not a skip.
    let mut records = case("A", 0, 1, "passed");
    records.push(record("A", "skipped", "skipped"));
    assert!(reduce_error(&records, &["A"], 0, 1).contains("one placeholder record"));
}

#[test]
fn warmup_fail_plus_measured_pass_is_one_failed_zero_passed() {
    let mut records = vec![record("A", "warmup", "failed")];
    records.extend(case("A", 0, 3, "passed"));
    let verdicts = reduce_sqlite_verdicts(&records, &ids(&["A"]), 1, 3).expect("reduce");
    assert_eq!(
        verdicts,
        Verdicts {
            passed: ids(&[]),
            failed: ids(&["A"]),
            skipped: ids(&[]),
        }
    );
    // One failed measured sample among passes fails the case too, and a
    // selection failure is a failed case with no samples.
    let mut records = case("B", 1, 3, "passed");
    records[2].status = "failed".to_owned();
    records.push(record("N", "not_run", "failed"));
    records.push(record("S", "skipped", "skipped"));
    records.extend(case("P", 1, 3, "passed"));
    let verdicts =
        reduce_sqlite_verdicts(&records, &ids(&["B", "N", "S", "P"]), 1, 3).expect("reduce");
    assert_eq!(verdicts.failed, ids(&["B", "N"]));
    assert_eq!(verdicts.skipped, ids(&["S"]));
    assert_eq!(verdicts.passed, ids(&["P"]));
    assert_eq!(verdicts.total(), 4);
}

#[test]
fn missing_or_replaced_case_same_total_rejected() {
    let mut records = case("A", 0, 1, "passed");
    records.extend(case("Z", 0, 1, "passed"));
    let error = reduce_error(&records, &["A", "B"], 0, 1);
    assert!(
        error.contains("missing [\"B\"]; not in the manifest [\"Z\"]"),
        "{error}"
    );
    // A case the manifest has and the records lack, with nothing in its
    // place.
    let error = reduce_error(&case("A", 0, 1, "passed"), &["A", "B"], 0, 1);
    assert!(
        error.contains("missing [\"B\"]; not in the manifest []"),
        "{error}"
    );
}

#[test]
fn case_verdicts_are_disjoint_without_a_plan() {
    let records = [
        record("A", "measured:1", "passed"),
        record("A", "measured:2", "failed"),
        record("B", "skipped", "skipped"),
        record("C", "measured:1", "passed"),
    ];
    let verdicts = case_verdicts(&records);
    assert_eq!(verdicts.failed, ids(&["A"]));
    assert_eq!(verdicts.skipped, ids(&["B"]));
    assert_eq!(verdicts.passed, ids(&["C"]));
}
