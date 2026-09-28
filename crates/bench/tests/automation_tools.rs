use std::{fs, path::Path, process::Command};

#[test]
fn score_policy_cli_rejects_regressions_with_golden_diagnostics() {
    let directory = tempfile::tempdir().unwrap();
    let before = directory.path().join("before.json");
    let after = directory.path().join("after.json");
    fs::write(
        &before,
        r#"{"score":99,"raw_score":100,"hard_findings":0,"soft_findings":1,"caps_applied":[]}"#,
    )
    .unwrap();
    fs::write(
        &after,
        r#"{"score":98,"raw_score":100,"hard_findings":1,"soft_findings":1,"caps_applied":["cap-a"]}"#,
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_score_policy"))
        .args(["compare"])
        .arg(&before)
        .arg(&after)
        .arg("push")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        concat!(
            "ERROR: score ratchet rejected this push:\n",
            "  - score decreased: 99 -> 98\n",
            "  - hard_findings increased: 0 -> 1\n",
            "  - finding_count increased: 1 -> 2\n",
            "  - applied cap count increased: 0 -> 1\n",
            "  - new applied caps: cap-a\n"
        )
    );
}

#[test]
fn perf_evidence_cli_emits_frozen_statistics_golden() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/perf-evidence/measured.jsonl");
    let output = Command::new(env!("CARGO_BIN_EXE_perf_evidence"))
        .arg("summarize-jsonl")
        .arg(&fixture)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        concat!(
            "  estimator:                  per-case ratio of medians (RedlineDB median / SQLite median elapsed ns, lower is better); median and nearest-rank p95 across eligible cases\n",
            "  repetitions:                not enforced (diagnostic)\n",
            "  cases:                      12\n",
            "  eligible cases:             10\n",
            "  failed cases:               1\n",
            "  skipped cases:              1\n",
            "  incomplete cases:           0\n",
            "  invalid rows:               0\n",
            "  faster cases:               0/10\n",
            "  case ratio median:          5.500\n",
            "  case ratio p95:             10.000\n",
            "  measured samples:           10\n",
            "  faster samples:             0/10\n",
            "  pooled sample ratio median: 5.500\n",
            "  pooled sample ratio p90:    9.900\n",
        )
    );

    // Publish mode with the fixture's one repetition, as JSON.
    let output = Command::new(env!("CARGO_BIN_EXE_perf_evidence"))
        .args(["summarize-jsonl", "--json", "--expected-repetitions", "1"])
        .arg(&fixture)
        .output()
        .unwrap();
    assert!(output.status.success());
    let summary: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["schema_version"], "perf-evidence-summary-v2");
    assert_eq!(summary["expected_repetitions"], 1);
    assert_eq!(summary["eligible_cases"], 10);
    assert_eq!(summary["faster_cases"], 0);
    assert_eq!(summary["measured_samples"], 10);
    assert_eq!(summary["faster_samples"], 0);
    assert_eq!(summary["invalid_rows"], 0);
    assert_eq!(summary["incomplete_cases"], 0);
    assert_eq!(summary["case_ratio_median"], 5.5);
}

#[test]
fn perf_evidence_cli_rejects_a_malformed_row() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("raw.jsonl");
    let fixture = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/perf-evidence/measured.jsonl"),
    )
    .unwrap();
    let mut lines = fixture.lines().collect::<Vec<_>>();
    lines.insert(1, "this malformed line used to be skipped silently");
    fs::write(&input, lines.join("\n")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_perf_evidence"))
        .arg("summarize-jsonl")
        .arg(&input)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("JSONL line 2: not JSON"), "{stderr}");
}
