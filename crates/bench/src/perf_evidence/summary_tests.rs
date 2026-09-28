//! Case-level performance statistics (R4-02 / BM3-06): the unit is the
//! case, every row parses, and each sample counts once.

use std::io::Cursor;

use super::*;

/// One measured sample of `case` in a run without warmups.
fn sample(
    case: &str,
    repetition: usize,
    reference_ns: u64,
    target_ns: u64,
    status: &str,
) -> String {
    serde_json::json!({
        "case_id": case,
        "status": status,
        "sample_role": format!("measured:{repetition}"),
        "sample_index": repetition - 1,
        "repetition_index": repetition,
        "reference_elapsed_ns": reference_ns,
        "target_elapsed_ns": target_ns,
        "latency_ratio": target_ns as f64 / reference_ns as f64,
    })
    .to_string()
}

fn passed(case: &str, repetition: usize, reference_ns: u64, target_ns: u64) -> String {
    sample(case, repetition, reference_ns, target_ns, "passed")
}

fn jsonl(lines: &[String]) -> String {
    lines.iter().map(|line| format!("{line}\n")).collect()
}

/// The value printed after `label:` on one line of `render()`.
fn rendered(render: &str, label: &str) -> Option<String> {
    render.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.trim() == label).then(|| value.trim().to_owned())
    })
}

fn summarize(input: &str) -> Result<JsonlSummary> {
    summarize_jsonl(Cursor::new(input.to_owned()))
}

#[test]
fn one_case_ratios_half_half_two_is_one_faster_case_and_two_faster_samples() {
    // Equal SQLite durations, RedlineDB at 0.5x, 0.5x and 2x: one case, and
    // its ratio of medians (500/1000) is a win; two of its three samples win.
    let input = jsonl(&[
        passed("10001", 1, 1_000, 500),
        passed("10001", 2, 1_000, 500),
        passed("10001", 3, 1_000, 2_000),
    ]);
    let render = summarize(&input).expect("summary").render();
    assert_eq!(
        rendered(&render, "eligible cases").as_deref(),
        Some("1"),
        "{render}"
    );
    assert_eq!(
        rendered(&render, "faster cases").as_deref(),
        Some("1/1"),
        "{render}"
    );
    assert_eq!(
        rendered(&render, "faster samples").as_deref(),
        Some("2/3"),
        "{render}"
    );
    assert_eq!(
        rendered(&render, "measured samples").as_deref(),
        Some("3"),
        "{render}"
    );
    assert_eq!(
        rendered(&render, "case ratio median").as_deref(),
        Some("0.500"),
        "{render}"
    );
}

#[test]
fn malformed_row_fails() {
    let input = format!("{}\nthis is not JSON\n", passed("10001", 1, 1_000, 900));
    let error = summarize(&input).expect_err("a malformed row is an error, never skipped");
    assert!(format!("{error:#}").contains("line 2"), "{error:#}");
    // A row that is JSON but not an object, and an object without the
    // fields a sample needs, fail the same way.
    for bad in ["[1, 2]", "{}", r#"{"case_id":"10001","status":"passed"}"#] {
        let input = format!("{}\n{bad}\n", passed("10001", 1, 1_000, 900));
        let error = summarize(&input).expect_err(bad);
        assert!(format!("{error:#}").contains("line 2"), "{bad}: {error:#}");
    }
}

#[test]
fn duplicate_repetition_fails() {
    let input = jsonl(&[
        passed("10001", 1, 1_000, 900),
        passed("10001", 2, 1_000, 900),
        passed("10001", 1, 1_000, 100),
    ]);
    let error = summarize(&input).expect_err("a repeated sample would reweight its case");
    let message = format!("{error:#}");
    assert!(
        message.contains("duplicate") && message.contains("line 3"),
        "{message}"
    );
}

#[test]
fn unequal_sample_counts_not_overweighted() {
    // Case A has ten fast samples, B and C one slow sample each. Pooling
    // samples would report a 0.5x median; per case it is 2x, and one case
    // of three is faster.
    let mut lines = (1..=10)
        .map(|repetition| passed("10001", repetition, 1_000, 500))
        .collect::<Vec<_>>();
    lines.push(passed("10002", 1, 1_000, 2_000));
    lines.push(passed("10003", 1, 1_000, 3_000));
    let render = summarize(&jsonl(&lines)).expect("summary").render();
    assert_eq!(
        rendered(&render, "case ratio median").as_deref(),
        Some("2.000"),
        "{render}"
    );
    assert_eq!(
        rendered(&render, "faster cases").as_deref(),
        Some("1/3"),
        "{render}"
    );
    assert_eq!(
        rendered(&render, "faster samples").as_deref(),
        Some("10/12"),
        "{render}"
    );
    assert_eq!(
        rendered(&render, "pooled sample ratio median").as_deref(),
        Some("0.500"),
        "{render}"
    );
}

#[test]
fn case_ratio_is_ratio_of_medians_not_median_of_sample_ratios() {
    // Sample ratios 1, 3 and 0.5 have median 1; the medians are 10 ns and
    // 30 ns, a ratio of 3. The two estimators are deliberately distinct.
    let input = jsonl(&[
        passed("10001", 1, 1, 1),
        passed("10001", 2, 10, 30),
        passed("10001", 3, 100, 50),
    ]);
    let render = summarize(&input).expect("summary").render();
    assert_eq!(
        rendered(&render, "case ratio median").as_deref(),
        Some("3.000"),
        "{render}"
    );
    assert_eq!(
        rendered(&render, "pooled sample ratio median").as_deref(),
        Some("1.000"),
        "{render}"
    );
}

fn publish(input: &str, repetitions: usize) -> Result<JsonlSummary> {
    summarize_jsonl_with(
        Cursor::new(input.to_owned()),
        SummaryOptions {
            expected_repetitions: Some(repetitions),
        },
    )
}

#[test]
fn missing_repetition_fails_publish() {
    // Case 10002 lost measured:2. A diagnostic summary excludes it as
    // incomplete; publish mode refuses the run.
    let input = jsonl(&[
        passed("10001", 1, 1_000, 900),
        passed("10001", 2, 1_000, 900),
        passed("10001", 3, 1_000, 900),
        passed("10002", 1, 1_000, 900),
        passed("10002", 3, 1_000, 900),
    ]);
    let summary = summarize(&input).expect("diagnostic summary");
    assert_eq!(
        (summary.eligible_cases, summary.incomplete_cases),
        (1, 1),
        "{}",
        summary.render()
    );
    let error = publish(&input, 3).expect_err("a missing repetition cannot be published");
    let message = format!("{error:#}");
    assert!(
        message.contains("case 10002") && message.contains("expected measured repetitions 1..=3"),
        "{message}"
    );
    // A case with more repetitions than the run declares fails too.
    let extra = format!("{input}{}\n", passed("10001", 4, 1_000, 900));
    assert!(publish(&extra, 3).is_err());
    // The complete run publishes.
    let complete = format!("{input}{}\n", passed("10002", 2, 1_000, 900));
    let summary = publish(&complete, 3).expect("complete run");
    assert_eq!(summary.expected_repetitions, Some(3));
    assert_eq!(summary.eligible_cases, 2);
    assert_eq!(summary.measured_samples, 6);
}

#[test]
fn failed_repetition_excludes_its_case_and_is_counted() {
    // One failed repetition fails the case: it is excluded from every
    // ratio and reported, in publish mode as in diagnostic mode.
    let input = jsonl(&[
        passed("10001", 1, 1_000, 2_000),
        passed("10001", 2, 1_000, 2_000),
        passed("10002", 1, 1_000, 100),
        sample("10002", 2, 1_000, 100, "failed"),
    ]);
    let summary = publish(&input, 2).expect("publishable with the failure reported");
    assert_eq!(
        (
            summary.eligible_cases,
            summary.failed_cases,
            summary.faster_cases
        ),
        (1, 1, 0)
    );
    assert_eq!(summary.case_ratio_median, Some(2.0));
    assert_eq!(summary.measured_samples, 2);
    // A run where nothing is eligible has nothing to publish.
    let only_failed = jsonl(&[
        sample("10002", 1, 1_000, 100, "failed"),
        passed("10002", 2, 1_000, 100),
    ]);
    let error = publish(&only_failed, 2).expect_err("empty eligible population");
    assert!(
        format!("{error:#}").contains("no eligible case"),
        "{error:#}"
    );
}

#[test]
fn unusable_duration_is_an_invalid_row() {
    // A zero duration or a recorded ratio that is not target/reference is
    // an invalid row: its case is excluded from a diagnostic summary and a
    // publish-mode summary refuses the run, naming the line.
    for bad in [
        passed("10002", 1, 0, 900),
        serde_json::json!({
            "case_id": "10002", "status": "passed", "sample_role": "measured:1",
            "sample_index": 0, "repetition_index": 1,
            "reference_elapsed_ns": 1_000, "target_elapsed_ns": 900, "latency_ratio": 0.1,
        })
        .to_string(),
    ] {
        let input = jsonl(&[passed("10001", 1, 1_000, 900), bad]);
        let summary = summarize(&input).expect("diagnostic summary");
        assert_eq!(
            (
                summary.invalid_rows,
                summary.incomplete_cases,
                summary.eligible_cases
            ),
            (1, 1, 1),
            "{}",
            summary.render()
        );
        let error = publish(&input, 1).expect_err("invalid row in publish mode");
        assert!(format!("{error:#}").contains("line 2"), "{error:#}");
    }
}

#[test]
fn p95_is_nearest_rank_across_cases() {
    // Twenty cases at 1x..20x, one sample each: the nearest-rank p95 is the
    // 19th smallest case ratio.
    let lines = (1..=20)
        .map(|k| passed(&format!("{:05}", 10_000 + k), 1, 1_000, 1_000 * k))
        .collect::<Vec<_>>();
    let summary = summarize(&jsonl(&lines)).expect("summary");
    assert_eq!(summary.case_ratio_p95, Some(19.0));
    assert_eq!(summary.case_ratio_median, Some(10.5));
}

#[test]
fn not_run_rows_are_single_and_never_measured() {
    let skipped = serde_json::json!({
        "case_id": "10009", "status": "skipped", "sample_role": "skipped",
    })
    .to_string();
    let summary =
        summarize(&jsonl(&[passed("10001", 1, 1_000, 900), skipped.clone()])).expect("summary");
    assert_eq!((summary.cases, summary.skipped_cases), (2, 1));
    // A skipped case that also carries a sample is no case the runner writes.
    let error = summarize(&jsonl(&[skipped, passed("10009", 1, 1_000, 900)]))
        .expect_err("not-run row plus sample");
    assert!(format!("{error:#}").contains("case 10009"), "{error:#}");
    // Unknown roles and skipped samples are errors, not rows to drop.
    for bad in [
        r#"{"case_id":"10001","status":"passed","sample_role":"measured-1","latency_ratio":1}"#,
        r#"{"case_id":"10001","status":"skipped","sample_role":"measured:1","repetition_index":1,"reference_elapsed_ns":1,"target_elapsed_ns":1}"#,
    ] {
        assert!(summarize(&format!("{bad}\n")).is_err(), "{bad}");
    }
}
