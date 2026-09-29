//! Publishing a summary: a missing repetition fails it, and a failed
//! repetition drops its case and is counted.

use super::*;

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
