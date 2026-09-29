//! What the version-history block refuses: a missing README or one without
//! markers, a bundle that is not publishable, narrowed or changed after its
//! summary, a malformed summary, and a bundle outside the README's tree.

use super::*;

#[test]
fn a_readme_without_the_markers_is_refused_and_left_alone() {
    let fixture = Fixture::new(3);
    for text in [
        "# RedlineDB\n\nNo table yet.\n".to_owned(),
        format!("{BEGIN}\n{END}\n{BEGIN}\n{END}\n"),
        format!("{END}\n{BEGIN}\n"),
    ] {
        fs::write(fixture.readme(), &text).expect("write");
        assert!(run(fixture.options(false)).is_err(), "{text:?}");
        assert_eq!(fs::read_to_string(fixture.readme()).expect("read"), text);
    }
}

#[test]
fn a_bundle_that_is_not_publishable_never_reaches_the_readme() {
    let fixture = Fixture::new(1);
    let error = format!("{:#}", run(fixture.options(false)).expect_err("K=1"));
    assert!(
        error.contains("not publishable") && error.contains("1 run(s) per label"),
        "{error}"
    );
    assert_eq!(fs::read_to_string(fixture.readme()).expect("read"), README);

    // Printed without --readme, it says so and shows no ranges.
    let summary = load_summary(&fixture.bundle()).expect("summary");
    let block = render_block(&summary, "bundle").expect("render");
    assert!(
        block.contains("> **Not publishable:** 1 run(s) per label"),
        "{block}"
    );
    assert!(block.contains("(of 2445, today's corpus)"), "{block}");
    assert!(
        block.contains("| 1.600× | 2.100× | not assessed (1 run) |"),
        "{block}"
    );
}

#[test]
fn a_narrowed_bundle_does_not_claim_the_corpus() {
    let fixture = Fixture::new(3);
    fixture.edit_summary(|summary| {
        summary["corpus"] = json!({"cases": 20, "narrowed_by": {"path": "case-list.txt"}});
        summary["publishable"] = json!(false);
        summary["publication_blockers"] = json!(["the corpus is narrowed to 20 cases"]);
    });
    let summary = load_summary(&fixture.bundle()).expect("summary");
    let block = render_block(&summary, "bundle").expect("render");
    assert!(
        block.contains("SQLite corpus passed (of 20 listed cases)"),
        "{block}"
    );
    assert!(!block.contains("today's corpus"), "{block}");
    assert!(
        block.contains("The corpus was narrowed by a case list."),
        "{block}"
    );
}

#[test]
fn a_raw_file_changed_after_the_summary_is_refused() {
    let fixture = Fixture::new(3);
    fs::write(fixture.bundle().join("v5.0.0/run-2/raw.jsonl"), "{}\n").expect("tamper");
    let error = format!("{:#}", run(fixture.options(false)).expect_err("tampered"));
    assert!(
        error.contains("not the") && error.contains("rerun perf_evidence summarize-bundle"),
        "{error}"
    );
    assert_eq!(fs::read_to_string(fixture.readme()).expect("read"), README);
}

#[test]
fn malformed_summaries_are_refused() {
    let cases: [Breakage; 5] = [
        ("schema", |summary| {
            summary["schema_version"] = json!("redline-release-bench-summary-v0");
        }),
        ("label", |summary| {
            summary["labels"][0]["label"] = json!("v4 | injected");
        }),
        ("commit", |summary| {
            summary["labels"][0]["source_commit"] = json!("`rm`");
        }),
        ("runs", |summary| {
            summary["runs_per_label"] = json!(4);
        }),
        ("path", |summary| {
            summary["labels"][0]["runs"][0]["raw"] = json!("../outside.jsonl");
        }),
    ];
    for (name, edit) in cases {
        let fixture = Fixture::new(3);
        fixture.edit_summary(edit);
        assert!(run(fixture.options(false)).is_err(), "{name}");
        assert_eq!(
            fs::read_to_string(fixture.readme()).expect("read"),
            README,
            "{name}"
        );
    }
}

#[test]
fn the_bundle_must_live_under_the_readme() {
    let fixture = Fixture::new(3);
    let elsewhere = TempDir::new().expect("tempdir");
    fs::write(elsewhere.path().join("README.md"), README).expect("write");
    let error = format!(
        "{:#}",
        run(VersionHistoryOptions {
            bundle: fixture.bundle(),
            readme: Some(elsewhere.path().join("README.md")),
            check: false,
        })
        .expect_err("outside")
    );
    assert!(error.contains("is not inside"), "{error}");
}

#[test]
fn check_needs_a_readme() {
    let fixture = Fixture::new(3);
    let error = run(VersionHistoryOptions {
        bundle: fixture.bundle(),
        readme: None,
        check: true,
    })
    .expect_err("check without readme");
    assert!(format!("{error:#}").contains("give --readme"));
}
