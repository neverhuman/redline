//! The runner's evidence writers against their readers: what
//! write_sqlite_parity_evidence and write_official_evidence emit is what
//! evidence_processor and `report` read, key for key (SQ-02b, SQ-03).
//!
//! The reader-side tests build their JSON by hand, restating the reader's
//! own assumptions; these tests write the real files from a scratch run and
//! render them.

#![cfg(unix)]

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::identity::{RunIdentity, capture_in};
use super::{
    EvidenceConfig, OfficialEvidenceConfig, OfficialSuiteEvidence, RUN_PROVENANCE_SCHEMA,
    write_official_evidence, write_sqlite_parity_evidence,
};
use crate::report::{ReportOptions, generate};
use crate::sqlite_parity::{
    BaselineSource, CaseFailure, KNOWN_FAILURES_SCHEMA, RunSummary, VerdictReason,
};

const SQLITE_VERSION: &str = "3.53.1 2026-05-05 10:34:17 c88b22011a54 (64-bit)";
const ORACLE_STAMP: &str =
    "36ca143645cf76997d07b66e9244c636b8ccdec64a1d50558259c4e415e6558b\n-O2 -DSQLITE_ENABLE_FTS5";

fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}

fn script(path: &Path, version: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("bin dir");
    fs::write(path, format!("#!/bin/sh\nprintf '%s\\n' {version:?}\n")).expect("script");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod");
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?}");
}

/// A scratch run: a stamped pinned-layout sqlite3, a target, and a clean
/// checkout the run identity is taken in.
struct Run {
    root: tempfile::TempDir,
    sqlite: PathBuf,
    target: PathBuf,
    identity: RunIdentity,
}

impl Run {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("redline-testing-official-evidence-")
            .tempdir()
            .expect("temp root");
        let prefix = root.path().join("sqlite-reference/3.53.1");
        let sqlite = prefix.join("bin/sqlite3");
        script(&sqlite, SQLITE_VERSION);
        fs::write(
            prefix.join(".sqlite-reference-sha3"),
            format!("{ORACLE_STAMP}\n"),
        )
        .expect("stamp");
        let target = root.path().join("release/redlinedb");
        script(&target, "redlinedb v5.0.0 test");
        let checkout = root.path().join("checkout");
        fs::create_dir_all(checkout.join("crates")).expect("checkout");
        fs::write(checkout.join("crates/lib.rs"), "fn a() {}\n").expect("source");
        git(&checkout, &["init", "--quiet"]);
        git(&checkout, &["add", "."]);
        git(&checkout, &["commit", "--quiet", "-m", "measured"]);
        let identity = capture_in(&checkout, &sqlite);
        Self {
            root,
            sqlite,
            target,
            identity,
        }
    }

    fn out(&self) -> PathBuf {
        self.root.path().join("out")
    }

    /// `case_id` measured three times with the identities the runner
    /// records on every sample.
    fn records(&self, case_id: &str, status: &str) -> String {
        let sqlite_sha = sha256_hex(fs::read(&self.sqlite).expect("sqlite3"));
        let target_sha = sha256_hex(fs::read(&self.target).expect("target"));
        (1..=3usize)
            .map(|repetition| {
                let record = json!({
                    "case_id": case_id, "name": format!("CASE_{case_id}"),
                    "case_file": format!("CASE_{case_id}.rs"), "priority": "P0",
                    "profile": "memory", "category": "SMOKE",
                    "sample_role": format!("measured:{repetition}"),
                    "repetition_index": repetition, "sample_index": repetition - 1,
                    "status": status,
                    "verdict_reason": if status == "passed" { "passed" } else { "target_semantic_failure" },
                    "reference_executable_sha256": sqlite_sha,
                    "target_executable_sha256": target_sha,
                    "reference_version": SQLITE_VERSION,
                    "reference_elapsed_ns": 2_000_000u64, "target_elapsed_ns": 4_000_000u64,
                });
                format!("{record}\n")
            })
            .collect()
    }

    /// One passing beyond_sqlite feature record: no timings, no sqlite3.
    fn beyond_records(&self) -> String {
        let target_sha = sha256_hex(fs::read(&self.target).expect("target"));
        let record = json!({
            "case_id": "BEYOND-001", "name": "FEATURE", "case_file": "BEYOND-001.rs",
            "priority": "P0", "profile": "beyond_sqlite", "category": "FEATURE",
            "sample_role": "measured:1", "repetition_index": 1, "status": "passed",
            "target_executable_sha256": target_sha,
            "reference_elapsed_ns": 0, "target_elapsed_ns": 0,
        });
        format!("{record}\n")
    }

    /// Writes one sqlite_parity suite (00001 passed, 00002 a listed
    /// failure), a beyond_sqlite suite, and the run's official-evidence.json.
    fn write(&self) -> (Value, Value) {
        let out = self.out();
        fs::create_dir_all(&out).expect("out dir");
        let raw_path = out.join("sqlite_parity.raw.jsonl");
        let raw = format!(
            "{}{}",
            self.records("00001", "passed"),
            self.records("00002", "failed")
        );
        fs::write(&raw_path, &raw).expect("raw");
        let marker = json!({
            "schema_version": "redline-testing-raw-complete-v1", "suite": "sqlite_parity",
            "raw_file": "sqlite_parity.raw.jsonl", "records": 6, "cases": 2,
            "raw_sha256": sha256_hex(&raw),
        });
        fs::write(
            out.join("sqlite_parity.raw.jsonl.complete.json"),
            format!("{marker}\n"),
        )
        .expect("marker");
        let mut summary = RunSummary {
            elapsed: Duration::from_millis(7),
            ..RunSummary::default()
        };
        summary.record_run("00001".to_owned(), None);
        summary.record_run(
            "00002".to_owned(),
            Some(CaseFailure {
                case_id: "00002".to_owned(),
                name: "CASE_00002".to_owned(),
                verdict_reasons: [VerdictReason::TargetSemanticFailure].into(),
            }),
        );
        write_sqlite_parity_evidence(EvidenceConfig {
            suite: "sqlite_parity".to_owned(),
            output: raw_path.clone(),
            target_bin: self.target.clone(),
            sqlite_bin: self.sqlite.clone(),
            tmp_root: self.root.path().join("tmp"),
            workers: "2".to_owned(),
            repetitions: 3,
            warmup: 0,
            memory_samples: false,
            measurement_order: Default::default(),
            command_line: vec!["redline-testing".to_owned(), "run".to_owned()],
            started_unix_ms: 1,
            ended_unix_ms: 2,
            summary: summary.clone(),
            run_identity: self.identity.clone(),
        })
        .expect("suite evidence");
        for name in [
            "all.jsonl",
            "all-manifest.json",
            "postgres-qualification.json",
            "postgres-progress.md",
            "beyond-sqlite-summary.json",
            "beyond-sqlite-ranked.csv",
            "beyond-sqlite-manifest.json",
            "beyond-sqlite-provenance.json",
        ] {
            fs::write(out.join(name), "{}\n").expect("run artifact");
        }
        fs::write(out.join("beyond_sqlite.raw.jsonl"), self.beyond_records()).expect("beyond raw");
        let beyond_summary = RunSummary {
            total: 1,
            passed: 1,
            ..RunSummary::default()
        };
        let baseline = self.root.path().join("known-failures.json");
        fs::write(&baseline, "{\"listed\": [\"00002\"]}\n").expect("baseline");
        write_official_evidence(OfficialEvidenceConfig {
            output_dir: out.clone(),
            all_output: out.join("all.jsonl"),
            all_manifest: out.join("all-manifest.json"),
            target_bin: self.target.clone(),
            sqlite_bin: self.sqlite.clone(),
            tmp_root: self.root.path().join("tmp"),
            workers: "2".to_owned(),
            repetitions: 3,
            warmup: 0,
            memory_samples: false,
            command_line: vec![
                "redline-testing".to_owned(),
                "run".to_owned(),
                "--suite".to_owned(),
                "all".to_owned(),
            ],
            generated_at_unix_ms: 3,
            suites: vec![
                OfficialSuiteEvidence::new(
                    "sqlite_parity",
                    raw_path,
                    out.join("summary.json"),
                    out.join("ranked.csv"),
                    out.join("manifest.json"),
                    out.join("provenance.json"),
                    &summary,
                )
                .with_known_failures(BTreeSet::from(["00002".to_owned()]))
                .with_completion_marker(),
                OfficialSuiteEvidence::new(
                    "beyond_sqlite",
                    out.join("beyond_sqlite.raw.jsonl"),
                    out.join("beyond-sqlite-summary.json"),
                    out.join("beyond-sqlite-ranked.csv"),
                    out.join("beyond-sqlite-manifest.json"),
                    out.join("beyond-sqlite-provenance.json"),
                    &beyond_summary,
                )
                .without_case_ids(),
            ],
            known_failures: Some(BaselineSource {
                path: baseline.clone(),
                sha256: sha256_hex(fs::read(&baseline).expect("baseline")),
            }),
            scope_policy_sha256: "5c09e".repeat(12) + "5c09",
            official: true,
            case_timeout_ms: 60_000,
            max_output_bytes: 1024,
            run_identity: self.identity.clone(),
        })
        .expect("official evidence");
        let read = |name: &str| -> Value {
            serde_json::from_str(&fs::read_to_string(out.join(name)).expect(name)).expect("json")
        };
        (read("official-evidence.json"), read("provenance.json"))
    }
}

#[test]
fn official_evidence_carries_what_the_processor_and_report_read() {
    let run = Run::new();
    let (official, provenance) = run.write();

    // The oracle build stamp beside the pinned shell, not merely a key.
    assert_eq!(
        provenance["oracle_build_stamp"], ORACLE_STAMP,
        "{provenance}"
    );
    assert_eq!(official["oracle_build_stamp"], ORACLE_STAMP, "{official}");
    // Both files record the one identity taken when the run started.
    for key in [
        "source_commit",
        "source_tree",
        "source_inputs_sha256",
        "source_dirty",
        "corpus_sha256",
        "assertion_policy_sha256",
    ] {
        assert!(!official[key].is_null(), "{key}: {official}");
        assert_eq!(official[key], provenance[key], "{key}");
    }
    assert_eq!(official["source_dirty"], false);
    assert_eq!(official["run_provenance_schema"], RUN_PROVENANCE_SCHEMA);
    assert_eq!(provenance["schema_version"], RUN_PROVENANCE_SCHEMA);

    // What evidence_processor holds to the known-failures baseline.
    let suite = &official["suites"]["sqlite_parity"];
    assert_eq!(suite["failed"], 1, "{suite}");
    assert_eq!(suite["failed_case_ids"], json!(["00002"]), "{suite}");
    assert_eq!(suite["known_failure_ids"], json!(["00002"]), "{suite}");
    assert_eq!(
        suite["completion_path"],
        "sqlite_parity.raw.jsonl.complete.json"
    );
    let baseline = &official["sqlite_known_failures"];
    assert_eq!(
        baseline["schema_version"], KNOWN_FAILURES_SCHEMA,
        "{baseline}"
    );
    let baseline_file = fs::read(run.root.path().join("known-failures.json")).expect("baseline");
    assert_eq!(baseline["sha256"], sha256_hex(baseline_file), "{baseline}");
    assert_eq!(official["run_mode"], "official");
    assert_eq!(official["status"], "failed");
    // The provenance file is bound by its hash.
    let provenance_bytes = fs::read(run.out().join("provenance.json")).expect("provenance");
    assert_eq!(
        official["output_file_hashes"]["provenance.json"],
        sha256_hex(&provenance_bytes)
    );
}

#[test]
fn report_accepts_the_runners_own_evidence_in_official_mode() {
    let run = Run::new();
    let (official, _) = run.write();
    let out = run.out();
    let raw = fs::read_to_string(out.join("sqlite_parity.raw.jsonl")).expect("raw");
    let run_text = fs::read(out.join("official-evidence.json")).expect("official");
    let marker: Value = serde_json::from_str(
        &fs::read_to_string(out.join("sqlite_parity.raw.jsonl.complete.json")).expect("marker"),
    )
    .expect("marker json");
    // The shape evidence_processor gives it: the run's file nested whole,
    // its hash, and the suite summary with the provenance hash and marker.
    let processed = json!({
        "schema_version": "redline-testing-official-evidence-processed-v1",
        "source_sha256": sha256_hex(&run_text),
        "status": official["status"],
        "official_evidence": official,
        "suite_summaries": {
            "sqlite_parity": {
                "total": 2, "passed": 1, "failed": 1, "skipped": 0,
                "raw_path": "sqlite_parity.raw.jsonl",
                "raw_sha256": sha256_hex(&raw),
                "provenance_sha256": official["output_file_hashes"]["provenance.json"],
                "completion": marker,
            }
        },
    });
    let evidence = out.join("official-evidence.processed.json");
    fs::write(
        &evidence,
        serde_json::to_string_pretty(&processed).expect("json"),
    )
    .expect("write");
    let readme = run.root.path().join("README.md");
    fs::write(
        &readme,
        "<!-- sqlite-parity-report:begin -->\n<!-- sqlite-parity-report:end -->\n",
    )
    .expect("readme");
    let report_dir = run.root.path().join("report");
    generate(ReportOptions {
        suite: "sqlite_parity".to_owned(),
        input: out.join("sqlite_parity.raw.jsonl"),
        official_evidence: Some(evidence),
        run_provenance: Some(out.join("provenance.json")),
        historical_run: false,
        local_diagnostics: false,
        out_dir: report_dir.clone(),
        readme,
        plot: None,
        performance_histogram_plot: None,
        median_test_performance_plot: None,
        jankurai_score: None,
        updated_date: "2026-09-28".to_owned(),
        expected_repetitions: Some(3),
        expected_warmup: Some(0),
        check: false,
        case_manifest: Some(BTreeSet::from(["00001".to_owned(), "00002".to_owned()])),
    })
    .expect("report accepts the runner's own evidence");
    let report: Value = serde_json::from_str(
        &fs::read_to_string(report_dir.join("report-provenance.json")).expect("report provenance"),
    )
    .expect("report provenance json");
    assert_eq!(report["mode"], "official", "{report}");
    assert_eq!(
        report["measurement"]["run"]["oracle_build_stamp"],
        ORACLE_STAMP
    );
    assert_eq!(
        report["measurement"]["run"]["source_commit"],
        official["source_commit"]
    );
    assert_eq!(report["measurement"]["elapsed_ns"], 7_000_000u64);
}

#[test]
fn each_suite_names_the_schema_of_its_own_provenance() {
    let run = Run::new();
    let (official, _) = run.write();
    assert_eq!(
        official["suites"]["sqlite_parity"]["provenance_schema"], RUN_PROVENANCE_SCHEMA,
        "{official}"
    );
    assert_eq!(
        official["suites"]["beyond_sqlite"]["provenance_schema"],
        crate::beyond_sqlite::taxonomy::PROVENANCE_SCHEMA,
        "{official}"
    );
}

#[test]
fn report_refuses_beyond_sqlite_evidence_by_its_provenance_schema() {
    // beyond-sqlite-provenance.json is not a run provenance `report` can
    // bind, and it is not a run that predates run provenance either: the
    // report says so in both modes instead of blaming the flags.
    let run = Run::new();
    let (official, _) = run.write();
    let out = run.out();
    let raw = fs::read_to_string(out.join("beyond_sqlite.raw.jsonl")).expect("raw");
    let processed = json!({
        "schema_version": "redline-testing-official-evidence-processed-v1",
        "source_sha256": sha256_hex(fs::read(out.join("official-evidence.json")).expect("official")),
        "status": official["status"],
        "official_evidence": official,
        "suite_summaries": {
            "beyond_sqlite": {
                "total": 1, "passed": 1, "failed": 0, "skipped": 0,
                "raw_path": "beyond_sqlite.raw.jsonl",
                "raw_sha256": sha256_hex(&raw),
                "provenance_sha256": official["output_file_hashes"]["beyond-sqlite-provenance.json"],
            }
        },
    });
    let evidence = out.join("official-evidence.processed.json");
    fs::write(&evidence, processed.to_string()).expect("write");
    let readme = run.root.path().join("README.md");
    fs::write(
        &readme,
        "<!-- sqlite-parity-report:begin -->\n<!-- sqlite-parity-report:end -->\n",
    )
    .expect("readme");
    for (run_provenance, historical_run) in [
        (Some(out.join("beyond-sqlite-provenance.json")), false),
        (None, true),
    ] {
        let error = generate(ReportOptions {
            suite: "beyond_sqlite".to_owned(),
            input: out.join("beyond_sqlite.raw.jsonl"),
            official_evidence: Some(evidence.clone()),
            run_provenance,
            historical_run,
            local_diagnostics: false,
            out_dir: run.root.path().join("report"),
            readme: readme.clone(),
            plot: None,
            performance_histogram_plot: None,
            median_test_performance_plot: None,
            jankurai_score: None,
            updated_date: "2026-09-28".to_owned(),
            expected_repetitions: Some(1),
            expected_warmup: Some(0),
            check: false,
            case_manifest: None,
        })
        .expect_err("beyond_sqlite evidence has no run provenance report can bind");
        let message = format!("{error:#}");
        assert!(
            message.contains(crate::beyond_sqlite::taxonomy::PROVENANCE_SCHEMA)
                && message.contains("check-postgres"),
            "historical_run={historical_run}: {message}"
        );
    }
}
