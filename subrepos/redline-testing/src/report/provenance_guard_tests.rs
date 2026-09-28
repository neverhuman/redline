//! The report provenance hashes exactly what the report rendered, and every
//! fail-closed branch of the identity checks refuses on its own (SQ-03,
//! L-03, R4-03).

use std::fs;

use serde_json::Value;

use super::generate;
use super::provenance_tests::{official_error, render, render_official, setup};
use super::test_fixtures::{self as fixture, pretty};
use super::utils::sha256_hex;

#[test]
fn official_report_hashes_what_it_rendered() {
    let rendered = render_official(|_, _| {}, false).expect("official report");
    let provenance = rendered.report_provenance();
    let root = rendered.root.path();
    let evidence = fs::read(root.join("official-evidence.processed.json")).expect("evidence");
    assert_eq!(
        provenance["processed_evidence_sha256"],
        sha256_hex(&String::from_utf8(evidence).expect("utf8"))
    );
    // The run's own official-evidence.json, as the processor recorded it.
    assert_eq!(
        provenance["run_evidence_sha256"],
        "daac7524c76944c99fdaf6ac397034c9aa5634906f035771ba83d7fd10e54ccc"
    );
    let raw = fs::read_to_string(root.join("raw.jsonl")).expect("raw");
    assert_eq!(provenance["raw_sha256"], sha256_hex(&raw));
    let hashes = &provenance["output_file_hashes"];
    for name in ["raw.jsonl", "summary.json", "ranked.csv", "manifest.json"] {
        assert_eq!(
            hashes[name],
            sha256_hex(&rendered.out(name)),
            "{name}: {hashes}"
        );
    }
    // Only the generated block of the README, not the file.
    let readme = rendered.readme();
    let begin = "<!-- sqlite-parity-report:begin -->";
    let block = &readme[readme.find(begin).expect("begin") + begin.len()
        ..readme
            .find("<!-- sqlite-parity-report:end -->")
            .expect("end")];
    let key = format!("{}#sqlite-parity-report", root.join("README.md").display());
    assert_eq!(hashes[key.as_str()], sha256_hex(block), "{hashes}");
    assert_ne!(hashes[key.as_str()], sha256_hex(&readme));
    let renderer = &provenance["renderer"];
    assert_eq!(
        renderer["version"],
        format!("redline-testing {}", env!("CARGO_PKG_VERSION"))
    );
    let exe = fs::read(std::env::current_exe().expect("current exe")).expect("exe");
    assert_eq!(
        renderer["binary_sha256"],
        format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(&exe))
    );
}

#[test]
fn malformed_or_unknown_executable_identities_are_refused() {
    let raw = fixture::raw_case("00001");
    for (section, field, value, expected) in [
        (
            "sqlite",
            "sha256",
            Value::from("e99d817b"),
            "sqlite.sha256 is not a SHA-256 digest",
        ),
        (
            "target",
            "sha256",
            Value::from("z".repeat(64)),
            "target.sha256 is not a SHA-256 digest",
        ),
        (
            "runner",
            "binary_path",
            Value::Null,
            "runner.binary_path is missing or unknown",
        ),
        (
            "runner",
            "binary_path",
            Value::from("<unknown>"),
            "runner.binary_path is missing or unknown",
        ),
    ] {
        let mut evidence = fixture::historical_evidence(&raw);
        evidence["official_evidence"][section][field] = value.clone();
        let error = render(&raw, &evidence)
            .err()
            .unwrap_or_else(|| panic!("rendered with {section}.{field} = {value}"));
        assert!(
            format!("{error:#}").contains(expected),
            "{section}.{field}: {error:#}"
        );
    }
}

#[test]
fn run_provenance_of_another_schema_suite_or_raw_file_is_refused() {
    for (what, edit, expected) in [
        (
            "schema",
            Box::new(|provenance: &mut Value, _: &mut Value| {
                provenance["schema_version"] = "redline-testing-provenance-v1".into();
            }) as Box<dyn FnOnce(&mut Value, &mut Value)>,
            "run provenance schema_version",
        ),
        (
            "suite",
            Box::new(|provenance: &mut Value, _: &mut Value| {
                provenance["suite"] = "memory".into();
            }),
            "run provenance is for suite",
        ),
        (
            "raw hash",
            Box::new(|provenance: &mut Value, _: &mut Value| {
                provenance["output_file_hashes"]["sqlite_parity.raw.jsonl"] = "0".repeat(64).into();
            }),
            "run provenance output_file_hashes.sqlite_parity.raw.jsonl",
        ),
    ] {
        // Re-bound, so only the named check can refuse it.
        let error = official_error(edit, true);
        assert!(error.contains(expected), "{what}: {error}");
    }
}

#[test]
fn provenance_flags_must_match_the_evidence() {
    let raw = fixture::raw_case("00001");
    // --run-provenance for evidence that binds no run provenance.
    let historical = fixture::historical_evidence(&raw);
    let run_provenance = pretty(&fixture::run_provenance(&raw));
    let (_root, options) = setup(&raw, &historical, Some(&run_provenance));
    let error = generate(options).expect_err("no run provenance is bound");
    assert!(
        format!("{error:#}").contains("records no run_provenance_schema"),
        "{error:#}"
    );
    // Both flags at once.
    let evidence = fixture::official_evidence(&raw, &run_provenance);
    let (_root, mut options) = setup(&raw, &evidence, Some(&run_provenance));
    options.historical_run = true;
    let error = generate(options).expect_err("both flags");
    assert!(
        format!("{error:#}").contains("pass either --run-provenance or --historical-run, not both"),
        "{error:#}"
    );
}

#[test]
fn only_sqlite_shell_suites_hold_samples_to_the_sqlite_reference() {
    // beyond_sqlite samples describe a PostgreSQL oracle: their reference
    // fields are not sqlite3's, and only the target is checked.
    let raw = fixture::raw_case("00001")
        .replace(fixture::SQLITE_SHA256, &"b".repeat(64))
        .replace(fixture::SQLITE_VERSION, "PostgreSQL 16.15");
    let mut evidence = fixture::historical_evidence(&raw);
    let summaries = evidence["suite_summaries"]
        .as_object_mut()
        .expect("summaries");
    let entry = summaries.remove("sqlite_parity").expect("sqlite entry");
    summaries.insert("beyond_sqlite".to_owned(), entry);
    let (_root, mut options) = setup(&raw, &evidence, None);
    options.suite = "beyond_sqlite".to_owned();
    options.expected_repetitions = Some(3);
    generate(options).expect("a beyond_sqlite report checks only the target");

    // The same samples in a sqlite_parity report name the wrong reference.
    let evidence = fixture::historical_evidence(&raw);
    let error = render(&raw, &evidence).err().expect("wrong reference");
    assert!(
        format!("{error:#}").contains("reference_executable_sha256"),
        "{error:#}"
    );
}
