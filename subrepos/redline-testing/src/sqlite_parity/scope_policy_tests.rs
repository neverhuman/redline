use super::*;
use crate::sqlite_parity::runner::{CaseFailure, VerdictReason};
use crate::sqlite_parity::test_fixtures::unique_case;

fn entry(suite: &str, case_id: &str, kind: &str, expiry: &str) -> serde_json::Value {
    serde_json::json!({
        "suite": suite, "case_id": case_id, "name": "UNIQUE_CONSTRAINT_FAILED",
        "kind": kind, "reason": "r", "owner": "o", "expiry": expiry
    })
}

fn policy(entries: &[serde_json::Value]) -> Result<ScopePolicy> {
    let text = serde_json::json!({
        "schema_version": SCOPE_POLICY_SCHEMA, "description": "d", "exceptions": entries
    });
    ScopePolicy::parse(text.to_string().as_bytes())
}

#[test]
fn committed_policy_names_real_rql_cases() {
    let policy = ScopePolicy::compiled().expect("compiled scope policy");
    policy
        .check_corpus(&crate::sqlite_parity::all_cases().expect("corpus"))
        .expect("every entry names a corpus case");
    let rql = crate::sqlite_parity::rql_phase1_cases()
        .expect("rql cases")
        .into_iter()
        .map(|case| case.display_id())
        .collect::<BTreeSet<_>>();
    let listed = policy.listed("rql_phase1");
    assert!(!listed.is_empty());
    assert!(listed.is_subset(&rql), "{:?}", listed.difference(&rql));
    // Today no target capability gap is accepted in the shell suites.
    assert!(policy.listed("sqlite_parity").is_empty());
    assert!(policy.listed("memory").is_empty());
}

#[test]
fn malformed_policies_are_rejected() {
    for (entries, needle) in [
        (
            vec![entry(
                "beyond_sqlite",
                "10547",
                "target_capability",
                "2027-01-01",
            )],
            "suite must be one of",
        ),
        (
            vec![entry(
                "sqlite_parity",
                "10547x",
                "target_capability",
                "2027-01-01",
            )],
            "five-digit",
        ),
        (
            vec![entry("sqlite_parity", "10547", "rql_rewrite", "2027-01-01")],
            "only rql_phase1",
        ),
        (
            vec![entry("memory", "10547", "target_capability", "2027-13-01")],
            "not a YYYY-MM-DD date",
        ),
        (
            vec![entry("memory", "10547", "target_capability", "soon")],
            "not a YYYY-MM-DD date",
        ),
        (
            vec![
                entry("memory", "10547", "target_capability", "2027-01-01"),
                entry("memory", "10547", "target_capability", "2027-02-01"),
            ],
            "listed twice",
        ),
    ] {
        let error = policy(&entries).expect_err(needle);
        assert!(format!("{error:#}").contains(needle), "{needle}: {error:#}");
    }
    let mut empty_reason = entry("memory", "10547", "target_capability", "2027-01-01");
    empty_reason["reason"] = " ".into();
    assert!(policy(&[empty_reason]).is_err());
    let mut unknown_field = entry("memory", "10547", "target_capability", "2027-01-01");
    unknown_field["budget"] = 4.into();
    assert!(policy(&[unknown_field]).is_err());
    assert!(
        ScopePolicy::parse(br#"{"schema_version":"v0","description":"d","exceptions":[]}"#)
            .is_err()
    );
}

#[test]
fn an_exception_covers_only_its_suite_case_and_gap() {
    let policy =
        policy(&[entry("memory", "10547", "target_capability", "2027-01-01")]).expect("policy");
    let case = unique_case();
    assert_eq!(
        policy
            .covers("memory", &case, GapKind::TargetCapability)
            .as_deref(),
        Some("memory:10547")
    );
    assert_eq!(
        policy.covers("sqlite_parity", &case, GapKind::TargetCapability),
        None
    );
    assert_eq!(policy.covers("memory", &case, GapKind::RqlRewrite), None);
    let mut other = unique_case();
    other.id = 10548;
    assert_eq!(
        policy.covers("memory", &other, GapKind::TargetCapability),
        None
    );
    // The entry must name the case the corpus has under that id.
    let mut renamed = unique_case();
    renamed.name = "OTHER".to_owned();
    assert!(policy.check_corpus(&[renamed]).is_err());
    assert!(policy.check_corpus(&[]).is_err());
    policy.check_corpus(&[case]).expect("same case");
}

#[test]
fn expired_exceptions_are_refused() {
    let policy =
        policy(&[entry("memory", "10547", "target_capability", "2026-09-27")]).expect("policy");
    policy
        .check_expiry("2026-09-27")
        .expect("valid through its expiry day");
    let error = policy.check_expiry("2026-09-28").expect_err("expired");
    assert!(
        format!("{error:#}").contains("memory:10547 (expired 2026-09-27)"),
        "{error:#}"
    );
}

#[test]
fn a_listed_case_that_ran_fails_the_gate() {
    let policy =
        policy(&[entry("memory", "10547", "target_capability", "2027-01-01")]).expect("policy");
    let mut summary = RunSummary::default();
    summary.skipped_case_ids.push("10547".to_owned());
    policy.gate("memory", &summary).expect("skipped as listed");
    let mut summary = RunSummary::default();
    summary.passed_case_ids.push("10547".to_owned());
    let error = policy.gate("memory", &summary).expect_err("ran");
    assert!(
        format!("{error:#}").contains("remove the entries"),
        "{error:#}"
    );
    let mut summary = RunSummary::default();
    summary.failures.push(CaseFailure {
        case_id: "10547".to_owned(),
        name: "UNIQUE_CONSTRAINT_FAILED".to_owned(),
        verdict_reasons: BTreeSet::from([VerdictReason::TargetSemanticFailure]),
    });
    assert!(policy.gate("memory", &summary).is_err());
    // Other suites are not affected by a memory entry.
    policy
        .gate("sqlite_parity", &summary)
        .expect("not listed there");
}

#[test]
fn civil_dates_are_proleptic_gregorian() {
    assert_eq!(civil_date(0), "1970-01-01");
    assert_eq!(civil_date(11_017), "2000-03-01");
    assert_eq!(civil_date(19_782), "2024-02-29");
    assert_eq!(civil_date(20_723), "2026-09-27");
    assert!(is_iso_date(&today_utc()));
}
