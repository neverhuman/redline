//! Gate tests over synthetic bundles. A bundle exercises artifact
//! verification without a live server; it is not compatibility evidence.
use std::collections::BTreeMap;
use std::path::PathBuf;

use super::cases::{CaseContract, agreeing_rows, contracts};
use super::*;

const PIN: &str = "sha256:efdf07c2f9d4df592783dcc8ea5f6db02efbf5f6452b527225ff5e58364570e9";

fn corpus_contracts() -> BTreeMap<String, CaseContract> {
    contracts(&super::super::oracle::load_cases().unwrap())
}

/// An agreeing run over the whole corpus, with `edit` applied to each
/// target row.
fn bundle_with(edit: impl Fn(&str, &mut Value)) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let mut raw = String::new();
    for (id, contract) in corpus_contracts() {
        let rows = agreeing_rows(&id, &contract);
        let (oracle, target) = rows.trim_end().split_once('\n').unwrap();
        let mut target: Value = serde_json::from_str(target).unwrap();
        edit(&id, &mut target);
        raw.push_str(&format!("{oracle}\n{target}\n"));
    }
    let mut hashes = serde_json::Map::new();
    for (name, text) in [
        ("beyond_sqlite.raw.jsonl", raw.as_str()),
        ("beyond-sqlite-summary.json", "{}"),
        ("beyond-sqlite-ranked.csv", "header\n"),
        ("beyond-sqlite-coverage.csv", "header\n"),
        ("beyond-sqlite-manifest.json", "{}"),
    ] {
        fs::write(dir.path().join(name), text).unwrap();
        hashes.insert(name.into(), Value::String(hash(text.as_bytes())));
    }
    let provenance = serde_json::json!({
        "output_file_hashes": hashes,
        "redline_testing_binary_sha256": "a".repeat(64),
        "target_binary_sha256": "b".repeat(64),
        "corpus_sha256": hash(super::super::oracle::MANIFEST.as_bytes()),
        "source_commit": "c".repeat(40),
        "source_dirty": false,
        "reference": {
            "settings": "160015|C|C|UTC",
            "image_digest": PIN,
        }
    });
    fs::write(
        dir.path().join("beyond-sqlite-provenance.json"),
        provenance.to_string(),
    )
    .unwrap();
    dir
}

fn bundle() -> tempfile::TempDir {
    bundle_with(|_, _| {})
}

fn raw_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("beyond_sqlite.raw.jsonl")
}

fn report(dir: &tempfile::TempDir) -> Value {
    serde_json::from_slice(&fs::read(dir.path().join("postgres-qualification.json")).unwrap())
        .unwrap()
}

/// Writes a v2 regression policy with the given lists.
fn policy_file(
    dir: &Path,
    failed_cases: &[&str],
    declared_unsupported: &[&str],
    declared_rejections: Option<Vec<String>>,
) -> PathBuf {
    let path = dir.join("regression.json");
    let mut policy = serde_json::json!({
        "schema_version": "redline-postgres-regression-v2",
        "reason": "reviewed",
        "corpus_sha256": hash(super::super::oracle::MANIFEST.as_bytes()),
        "failed_cases": failed_cases,
        "declared_unsupported": declared_unsupported,
        "reference_settings": "160015|C|C|UTC",
        "image_digest": PIN,
        "server_binary_sha256": "d".repeat(64),
    });
    if let Some(declared) = declared_rejections {
        policy["declared_rejections"] = serde_json::json!(declared);
    }
    fs::write(&path, policy.to_string()).unwrap();
    path
}

fn baseline(dir: &Path, failed_cases: &[&str]) -> PathBuf {
    policy_file(dir, failed_cases, &[], None)
}

fn readme_in(dir: &tempfile::TempDir) -> PathBuf {
    let readme = dir.path().join("README.md");
    fs::write(
        &readme,
        "top\n<!-- POSTGRES_PARITY_START -->\nold\n<!-- POSTGRES_PARITY_END -->\nbottom\n",
    )
    .unwrap();
    readme
}

#[test]
fn a_baseline_failure_that_now_passes_fails_the_gate() {
    let dir = bundle();
    let raw = raw_path(&dir);
    // Every case in the bundle passes, so a baseline that claims one fails
    // is behind the live result. The subset ratchet alone is happy with this -- an empty set
    // is a subset of anything -- which is the hole being closed.
    let behind = baseline(dir.path(), &["BEYOND-CASE-20021"]);
    let err = check(&raw, Some(&behind), None).unwrap_err().to_string();
    assert!(
        err.contains("regression baseline lists failures that now pass"),
        "{err}"
    );
    assert!(err.contains("BEYOND-CASE-20021"), "{err}");

    // ... and the artifact records which ones, not just that some exist.
    assert_eq!(
        report(&dir)["newly_passing"],
        serde_json::json!(["BEYOND-CASE-20021"])
    );

    // An exact baseline still passes, and records nothing newly passing.
    let exact = baseline(dir.path(), &[]);
    check(&raw, Some(&exact), None).unwrap();
    assert_eq!(report(&dir)["newly_passing"], serde_json::json!([]));
}

#[test]
fn tampered_artifacts_fail_without_rewriting_the_receipt() {
    let dir = bundle();
    let raw = raw_path(&dir);
    check(&raw, None, None).unwrap();
    let provenance = dir.path().join("beyond-sqlite-provenance.json");
    let before = fs::read(&provenance).unwrap();
    fs::write(dir.path().join("beyond-sqlite-summary.json"), "tampered").unwrap();
    assert!(
        check(&raw, None, None)
            .unwrap_err()
            .to_string()
            .contains("artifact hash mismatch")
    );
    assert_eq!(fs::read(provenance).unwrap(), before);
}

#[test]
fn an_unknown_oracle_or_an_unasserted_error_cannot_pass() {
    let dir = bundle();
    let provenance = dir.path().join("beyond-sqlite-provenance.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&provenance).unwrap()).unwrap();
    value["reference"]["image_digest"] = Value::String("unknown".into());
    fs::write(provenance, value.to_string()).unwrap();
    assert!(
        check(&raw_path(&dir), None, None)
            .unwrap_err()
            .to_string()
            .contains("unknown PostgreSQL reference digest")
    );
    let contracts = corpus_contracts();
    let positive = contracts
        .iter()
        .find(|(_, contract)| contract.declared_error.is_none())
        .map(|(id, contract)| (id.clone(), contract.clone()))
        .unwrap();
    let only = BTreeMap::from([positive.clone()]);
    assert!(
        outcomes(
            &agreeing_rows(&positive.0, &positive.1).replace("exit_code\":0", "exit_code\":3"),
            &only
        )
        .is_err()
    );
}

#[test]
fn readme_block_reports_positive_and_rejection_split() {
    let dir = bundle();
    let readme = readme_in(&dir);
    check(&raw_path(&dir), None, Some(&readme)).unwrap();
    let text = fs::read_to_string(&readme).unwrap();
    assert!(
        text.starts_with("top\n<!-- POSTGRES_PARITY_START -->\n"),
        "{text}"
    );
    assert!(
        text.ends_with("<!-- POSTGRES_PARITY_END -->\nbottom\n"),
        "{text}"
    );
    for expected in [
        "(`redlinedb` CLI, `REDLINEDB_RESULT_DIALECT=postgres`, fresh `:memory:` per case)",
        "**265/265 agree** = **253** row matches + **12** expected rejections (declared error text verified)",
        "**0** declared unsupported; **0** mismatches; **0** skipped.",
        "Agreement is normalized SQL-shell transcript agreement, not typed-result or application parity.",
        "Not covered: wire protocol, TLS, roles/authorization, SQLSTATE, NOTIFY delivery, replication/CDC, extensions",
        "Source `cccccccccccccccccccccccccccccccccccccccc`; corpus SHA-256 `b240a7204eeb46893ea1f06e715a144f6cd962efe8f41041582e52ca975cd5be`.",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in {text}");
    }
    assert!(!text.contains("dirty"), "{text}");
    let report = report(&dir);
    assert_eq!(
        report["schema_version"],
        "redline-postgres-qualification-v2"
    );
    assert_eq!(report["positive_matches"], 253);
    assert_eq!(
        report["expected_rejections"].as_array().map(Vec::len),
        Some(12)
    );
    assert_eq!(report["passed"], 265);
    assert_eq!(report["comparison"], COMPARISON);
}

#[test]
fn a_declared_unsupported_refusal_is_reported_and_ratcheted() {
    let refused = "BEYOND-CASE-20435";
    let stderr = "Error: 1002: unsupported capability: pg_notify: no delivery\n";
    let dir = bundle_with(|id, row| {
        if id == refused {
            row["status"] = "failed".into();
            row["target_exit_code"] = 3.into();
            row["target_stderr"] = stderr.into();
            row["target_stderr_sha256"] = format!("{:x}", Sha256::digest(stderr)).into();
        }
    });
    let raw = raw_path(&dir);
    // Listed as declared unsupported: the gate passes, and says so.
    let listed = policy_file(dir.path(), &[], &[refused], None);
    let readme = readme_in(&dir);
    check(&raw, Some(&listed), Some(&readme)).unwrap();
    let report = report(&dir);
    assert_eq!(report["qualification"], "failed");
    assert_eq!(report["regression"], "passed");
    assert_eq!(report["declared_unsupported"], serde_json::json!([refused]));
    assert_eq!(report["mismatches"], serde_json::json!([]));
    assert_eq!(report["failed"], 1);
    assert_eq!(report["passed"], 264);
    let text = fs::read_to_string(&readme).unwrap();
    assert!(
        text.contains("**264/265 agree** = **252** row matches + **12** expected rejections"),
        "{text}"
    );
    assert!(
        text.contains("**1** declared unsupported; **0** mismatches"),
        "{text}"
    );
    // Listed only as an ordinary failure, or not at all: the gate fails.
    for policy in [
        policy_file(dir.path(), &[refused], &[], None),
        policy_file(dir.path(), &[], &[], None),
    ] {
        let err = check(&raw, Some(&policy), None).unwrap_err().to_string();
        assert!(
            err.contains("1 declared unsupported, 0 mismatches"),
            "{err}"
        );
    }
}

#[test]
fn a_policy_whose_declared_rejections_differ_from_the_corpus_fails() {
    let dir = bundle();
    let declared: Vec<String> = corpus_contracts()
        .into_iter()
        .filter(|(_, contract)| contract.declared_error.is_some())
        .map(|(id, _)| id)
        .collect();
    let exact = policy_file(dir.path(), &[], &[], Some(declared.clone()));
    check(&raw_path(&dir), Some(&exact), None).unwrap();
    let mut short = declared;
    short.pop();
    let stale = policy_file(dir.path(), &[], &[], Some(short));
    let err = check(&raw_path(&dir), Some(&stale), None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("but the corpus declares"), "{err}");
}

#[test]
fn the_committed_policy_accepts_an_agreeing_run() {
    let committed = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../metadata/beyond_sqlite/postgres-regression.json");
    let dir = bundle();
    check(&raw_path(&dir), Some(&committed), None).unwrap();
}
