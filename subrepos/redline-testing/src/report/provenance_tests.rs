//! Run and report provenance tests (SQ-03/L-03/R4-03): measured identities
//! come only from validated run evidence, never from the rendering host.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::generate;
use super::test_fixtures::{self as fixture, pretty};
use super::types::ReportOptions;
use super::utils::sha256_hex;

const README: &str =
    "# Report\n\n<!-- sqlite-parity-report:begin -->\n<!-- sqlite-parity-report:end -->\n";

pub(super) struct Rendered {
    pub(super) root: tempfile::TempDir,
}

impl Rendered {
    pub(super) fn out(&self, name: &str) -> String {
        fs::read_to_string(self.root.path().join("out").join(name)).expect("report output")
    }

    pub(super) fn report_provenance(&self) -> Value {
        serde_json::from_str(&self.out("report-provenance.json")).expect("report provenance")
    }

    pub(super) fn readme(&self) -> String {
        fs::read_to_string(self.root.path().join("README.md")).expect("readme")
    }
}

fn options(
    root: &Path,
    evidence: PathBuf,
    run_provenance: Option<PathBuf>,
    raw_text: &str,
) -> ReportOptions {
    ReportOptions {
        suite: "sqlite_parity".to_owned(),
        input: root.join("raw.jsonl"),
        official_evidence: Some(evidence),
        historical_run: run_provenance.is_none(),
        run_provenance,
        local_diagnostics: false,
        out_dir: root.join("out"),
        readme: root.join("README.md"),
        plot: None,
        performance_histogram_plot: None,
        median_test_performance_plot: None,
        jankurai_score: None,
        updated_date: "2026-09-24".to_owned(),
        expected_repetitions: Some(3),
        expected_warmup: Some(0),
        check: false,
        case_manifest: Some(fixture::case_ids(raw_text)),
    }
}

/// Writes one run (raw, evidence and, when given, run provenance) into a
/// fresh directory and returns it with the report options for it.
pub(super) fn setup(
    raw_text: &str,
    evidence: &Value,
    run_provenance: Option<&str>,
) -> (tempfile::TempDir, ReportOptions) {
    let root = tempfile::Builder::new()
        .prefix("redline-testing-provenance-")
        .tempdir()
        .expect("temp root");
    fs::write(root.path().join("raw.jsonl"), raw_text).expect("raw");
    fs::write(root.path().join("README.md"), README).expect("readme");
    let evidence_path = root.path().join("official-evidence.processed.json");
    fs::write(&evidence_path, pretty(evidence)).expect("evidence");
    let run_provenance = run_provenance.map(|text| {
        let path = root.path().join("run-provenance.json");
        fs::write(&path, text).expect("run provenance");
        path
    });
    let options = options(root.path(), evidence_path, run_provenance, raw_text);
    (root, options)
}

/// Renders a historical run (official evidence, no run provenance).
pub(super) fn render(raw_text: &str, evidence: &Value) -> anyhow::Result<Rendered> {
    let (root, options) = setup(raw_text, evidence, None);
    generate(options)?;
    Ok(Rendered { root })
}

/// Renders a run with run provenance; `edit` may alter the run provenance
/// and evidence first. `rebind` re-records the edited run provenance's hash
/// in the evidence, as a consistent forgery would.
pub(super) fn render_official(
    edit: impl FnOnce(&mut Value, &mut Value),
    rebind: bool,
) -> anyhow::Result<Rendered> {
    let raw = fixture::raw_case("00001");
    let mut run_provenance = fixture::run_provenance(&raw);
    let original = pretty(&run_provenance);
    let mut evidence = fixture::official_evidence(&raw, &original);
    edit(&mut run_provenance, &mut evidence);
    let text = pretty(&run_provenance);
    if rebind {
        evidence["suite_summaries"]["sqlite_parity"]["provenance_sha256"] =
            sha256_hex(&text).into();
    }
    let (root, options) = setup(&raw, &evidence, Some(&text));
    generate(options)?;
    Ok(Rendered { root })
}

pub(super) fn official_error(edit: impl FnOnce(&mut Value, &mut Value), rebind: bool) -> String {
    match render_official(edit, rebind) {
        Ok(_) => panic!("rendered an official report from inconsistent provenance"),
        Err(err) => format!("{err:#}"),
    }
}

#[test]
fn official_report_requires_the_runs_completion_marker() {
    // A run that stopped part way left readable records but no marker.
    let error = official_error(
        |_, evidence| {
            evidence["suite_summaries"]["sqlite_parity"]
                .as_object_mut()
                .expect("suite summary")
                .remove("completion");
        },
        false,
    );
    assert!(error.contains("records no completion marker"), "{error}");
    // A marker certifies its own bytes and record count, not these.
    let error = official_error(
        |_, evidence| {
            evidence["suite_summaries"]["sqlite_parity"]["completion"]["raw_sha256"] =
                "0".repeat(64).into();
        },
        false,
    );
    assert!(error.contains("certifies raw SHA-256"), "{error}");
    let error = official_error(
        |_, evidence| {
            evidence["suite_summaries"]["sqlite_parity"]["completion"]["records"] = 2.into();
        },
        false,
    );
    assert!(
        error.contains("certifies Some(Number(2)) records"),
        "{error}"
    );
    // With the marker intact the report renders.
    render_official(|_, _| {}, false).expect("complete run renders");
}

#[test]
fn report_rejects_unknown_identity() {
    let raw = fixture::raw_case("00001");
    for (section, field) in [
        ("runner", "binary_sha256"),
        ("runner", "version"),
        ("target", "path"),
        ("target", "sha256"),
        ("target", "version"),
        ("sqlite", "path"),
        ("sqlite", "sha256"),
        ("sqlite", "version"),
    ] {
        for replacement in [Value::from("<unknown>"), Value::Null] {
            let mut evidence = fixture::historical_evidence(&raw);
            evidence["official_evidence"][section][field] = replacement.clone();
            let err = render(&raw, &evidence).err().unwrap_or_else(|| {
                panic!("rendered with official_evidence.{section}.{field} = {replacement}")
            });
            assert!(
                format!("{err:#}").contains(&format!("{section}.{field}")),
                "{section}.{field}: {err:#}"
            );
        }
    }
}

#[test]
fn report_rejects_raw_identity_mismatch() {
    let clean = fixture::raw_case("00001");
    for (field, other) in [
        (
            "reference_executable_sha256",
            "fd3bdd25fd3bdd25fd3bdd25fd3bdd25fd3bdd25fd3bdd25fd3bdd25fd3bdd25",
        ),
        (
            "target_executable_sha256",
            "74f4869774f4869774f4869774f4869774f4869774f4869774f4869774f48697",
        ),
        (
            "reference_version",
            "3.45.1 2024-01-30 16:01:20 e876e51a0ed5 (64-bit)",
        ),
    ] {
        // One sample of the run names a different executable than the evidence.
        let raw = clean
            .lines()
            .enumerate()
            .map(|(index, line)| {
                let mut record: Value = serde_json::from_str(line).expect("raw record");
                if index == 1 {
                    record[field] = other.into();
                }
                format!("{record}\n")
            })
            .collect::<String>();
        let evidence = fixture::historical_evidence(&raw);
        let err = render(&raw, &evidence)
            .err()
            .unwrap_or_else(|| panic!("rendered a run whose raw {field} disagrees"));
        let message = format!("{err:#}");
        assert!(message.contains(field), "{field}: {message}");
        assert!(message.contains("00001"), "{field}: {message}");
    }
}

#[test]
fn historical_report_takes_identity_from_evidence() {
    let raw = fixture::raw_case("00001");
    let rendered = render(&raw, &fixture::historical_evidence(&raw)).expect("historical report");
    let provenance = rendered.report_provenance();
    assert_eq!(
        provenance["schema_version"],
        "redline-testing-report-provenance-v1"
    );
    assert_eq!(provenance["mode"], "historical");
    assert!(
        provenance["note"]
            .as_str()
            .is_some_and(|note| note.starts_with("historical run")),
        "{provenance:#}"
    );
    let measurement = &provenance["measurement"];
    assert_eq!(measurement["sqlite"]["sha256"], fixture::SQLITE_SHA256);
    assert_eq!(measurement["sqlite"]["version"], fixture::SQLITE_VERSION);
    assert_eq!(measurement["target"]["sha256"], fixture::TARGET_SHA256);
    assert_eq!(measurement["runner"]["sha256"], fixture::RUNNER_SHA256);
    assert!(measurement["run"].is_null(), "{provenance:#}");
    // The evidence names the run's provenance hash; the file is gone.
    let parent = &provenance["parent_run_provenance"];
    assert_eq!(parent["retained"], false);
    assert!(
        parent["sha256"]
            .as_str()
            .is_some_and(|sha| sha.starts_with("8de6a848"))
    );
    let summary: Value = serde_json::from_str(&rendered.out("summary.json")).expect("summary");
    assert!(
        summary["elapsed_ns"].is_null(),
        "no fabricated elapsed time"
    );
    let block = rendered.readme();
    assert!(
        block.contains("Historical run: it predates run provenance"),
        "{block}"
    );
    for name in ["summary.json", "manifest.json", "report-provenance.json"] {
        let text = rendered.out(name);
        assert!(!text.contains("<unknown>"), "{name}: {text}");
        assert!(!text.contains("git_"), "{name}: {text}");
    }
    assert!(
        !rendered.root.path().join("out/provenance.json").exists(),
        "the report must not write a file under the run provenance's name"
    );
}

#[test]
fn official_report_records_the_run_identity() {
    let rendered = render_official(|_, _| {}, false).expect("official report");
    let provenance = rendered.report_provenance();
    assert_eq!(provenance["mode"], "official");
    assert!(provenance["note"].is_null());
    let run = &provenance["measurement"]["run"];
    assert_eq!(run["source_tree"], fixture::SOURCE_TREE);
    assert_eq!(run["source_inputs_sha256"], fixture::SOURCE_INPUTS_SHA256);
    assert_eq!(run["source_dirty"], false);
    assert_eq!(run["corpus_sha256"], fixture::CORPUS_SHA256);
    assert_eq!(
        run["assertion_policy_sha256"],
        fixture::ASSERTION_POLICY_SHA256
    );
    assert_eq!(run["oracle_build_stamp"], fixture::ORACLE_BUILD_STAMP);
    assert_eq!(provenance["parent_run_provenance"]["retained"], true);
    let summary: Value = serde_json::from_str(&rendered.out("summary.json")).expect("summary");
    assert_eq!(summary["elapsed_ns"], fixture::RUN_ELAPSED_NS);
    let manifest: Value = serde_json::from_str(&rendered.out("manifest.json")).expect("manifest");
    assert!(manifest["output_files"]["run_provenance"].is_string());
    assert!(manifest["output_files"]["report_provenance"].is_string());
    let readme = rendered.readme();
    assert!(
        readme.contains("source tree `7ee7ee7ee7ee` (clean)"),
        "{readme}"
    );
    assert!(readme.contains("corpus SHA-256 `c0ffeec0ffee`"), "{readme}");
}

#[test]
fn report_rejects_swapped_source_inputs_hash() {
    let swap = |provenance: &mut Value, _: &mut Value| {
        provenance["source_inputs_sha256"] =
            "5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a".into();
    };
    // Unbound: the evidence records a different run provenance.
    let err = official_error(swap, false);
    assert!(
        err.contains("run provenance") && err.contains("SHA-256"),
        "{err}"
    );
    // Re-bound: the run provenance disagrees with official-evidence.json.
    let err = official_error(swap, true);
    assert!(err.contains("source_inputs_sha256"), "{err}");
}

#[test]
fn report_rejects_dirty_run() {
    let err = official_error(
        |provenance, evidence| {
            provenance["source_dirty"] = true.into();
            provenance["source_dirty_paths"] = serde_json::json!([" M crates/sql/src/exec/mod.rs"]);
            evidence["official_evidence"]["source_dirty"] = true.into();
        },
        true,
    );
    assert!(err.contains("dirty source tree"), "{err}");
    assert!(err.contains("crates/sql/src/exec/mod.rs"), "{err}");
}

#[test]
fn report_rejects_unrecorded_run_identity() {
    for key in [
        "source_tree",
        "source_inputs_sha256",
        "source_dirty",
        "corpus_sha256",
        "assertion_policy_sha256",
        "oracle_build_stamp",
    ] {
        let err = official_error(
            |provenance, evidence| {
                provenance[key] = Value::Null;
                evidence["official_evidence"][key] = Value::Null;
            },
            true,
        );
        assert!(err.contains(&format!("records no {key}")), "{key}: {err}");
    }
}

#[test]
fn report_rejects_run_provenance_for_another_executable() {
    let err = official_error(
        |provenance, _| {
            provenance["sqlite_binary_sha256"] =
                "fd3bdd25fd3bdd25fd3bdd25fd3bdd25fd3bdd25fd3bdd25fd3bdd25fd3bdd25".into();
        },
        true,
    );
    assert!(err.contains("sqlite_binary_sha256"), "{err}");
}

#[test]
fn historical_mode_is_refused_for_evidence_with_run_provenance() {
    let raw = fixture::raw_case("00001");
    let run_provenance = pretty(&fixture::run_provenance(&raw));
    let evidence = fixture::official_evidence(&raw, &run_provenance);
    let err = render(&raw, &evidence)
        .err()
        .expect("historical mode refused");
    assert!(
        format!("{err:#}").contains("pass --run-provenance"),
        "{err:#}"
    );

    // Neither flag: official evidence always needs one of them.
    let (_root, mut options) = setup(&raw, &evidence, None);
    options.historical_run = false;
    let err = generate(options).expect_err("no provenance mode");
    assert!(format!("{err:#}").contains("--run-provenance"), "{err:#}");
}
