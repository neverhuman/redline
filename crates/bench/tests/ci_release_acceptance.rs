//! CI-04/CI-06: a release names what it was accepted on. release-build.yml's
//! publish job downloads the receipt artifacts of the tag's acceptance run,
//! writes release-acceptance.v1.json (ops/ci/release-acceptance.sh), checks
//! it (scripts/release/verify-acceptance.sh), attests it with the archives and
//! uploads it as a release asset; the publisher refuses without it.
//!
//! These checks read the workflow text; ops/ci/tests/release-acceptance.sh
//! and ops/ci/tests/release-authority.sh exercise the scripts.

#[path = "support/workflow_text.rs"]
mod workflow_text;
use std::collections::BTreeSet;
use workflow_text::{job, jobs, read, run_shell_test, steps_using};

const MANIFEST: &str = "target/acceptance/release-acceptance.v1.json";

/// `(artifact name, comment)` for every receipt ops/release/acceptance-receipts lists.
fn receipts() -> Vec<(String, String)> {
    read("ops/release/acceptance-receipts")
        .lines()
        .filter(|line| !line.trim_start().starts_with('#') && !line.trim().is_empty())
        .map(|line| {
            let (name, comment) = line.split_once('#').unwrap_or((line, ""));
            (name.trim().to_string(), comment.trim().to_string())
        })
        .collect()
}

fn position(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("release-build.yml publish lacks `{needle}`"))
}

#[test]
fn publish_writes_checks_attests_and_uploads_the_manifest() {
    let release = read(".github/workflows/release-build.yml");
    let publish = job(&release, "publish");
    assert!(
        publish.contains("      actions: read\n"),
        "publish needs actions: read to list the run's jobs"
    );

    let pattern = publish
        .lines()
        .find_map(|line| line.trim().strip_prefix("pattern: '{"))
        .and_then(|rest| rest.strip_suffix("}'"))
        .expect("publish downloads the receipts with one brace pattern");
    let downloaded: BTreeSet<String> = pattern.split(',').map(str::to_string).collect();
    let bound: BTreeSet<String> = receipts().into_iter().map(|(name, _)| name).collect();
    assert_eq!(
        downloaded, bound,
        "publish must download exactly the receipts ops/release/acceptance-receipts binds"
    );

    let generate = position(
        &publish,
        &format!(
            "bash ops/ci/release-acceptance.sh target/packages target/acceptance/receipts {MANIFEST}"
        ),
    );
    let verify = position(
        &publish,
        &format!("bash scripts/release/verify-acceptance.sh {MANIFEST}"),
    );
    let attest = position(&publish, &format!("subject-path: {MANIFEST}"));
    let publication = position(&publish, "run: bash ops/ci/publish-github-release.sh");
    assert!(
        generate < verify && verify < attest && attest < publication,
        "publish must write, check and attest the manifest before publishing"
    );
    assert!(publish.contains(&format!("RELEASE_ACCEPTANCE: {MANIFEST}")));

    let publisher = read("ops/ci/publish-github-release.sh");
    assert!(publisher.contains("scripts/release/verify-acceptance.sh\" \"$acceptance\""));
    assert!(
        publisher
            .lines()
            .any(|line| line.starts_with("gh release upload") && line.ends_with("\"$acceptance\"")),
        "the manifest must be uploaded with the archives"
    );
}

#[test]
fn every_bound_receipt_has_an_uploader_or_says_it_has_none() {
    let ci = read(".github/workflows/ci.yml");
    let uploads: Vec<String> = jobs(&ci)
        .into_iter()
        .flat_map(|(_, body)| steps_using(&body, "actions/upload-artifact@"))
        .collect();
    let missing_note = "no ci.yml job uploads it yet";
    for (name, comment) in receipts() {
        let uploaded = uploads.iter().any(|step| {
            step.contains(&format!("name: {name},")) || step.contains(&format!("name: {name}\n"))
        });
        if uploaded {
            assert!(
                !comment.contains(missing_note),
                "ci.yml uploads {name}; update its line in ops/release/acceptance-receipts"
            );
        } else {
            assert!(
                comment.contains(missing_note),
                "no ci.yml job uploads the receipt {name}; upload it or say so in ops/release/acceptance-receipts"
            );
        }
    }
}

#[test]
fn the_security_job_writes_the_receipt_from_a_full_clone() {
    let ci = read(".github/workflows/ci.yml");
    let security = job(&ci, "security");
    let checkout = &steps_using(&security, "actions/checkout@")[0];
    assert!(
        checkout.contains("fetch-depth: 0"),
        "the receipt's gitleaks scan covers the whole history"
    );
    assert!(security.contains("run: bash ops/ci/security-receipt.sh"));
}

#[test]
fn the_manifest_scripts_fail_closed() {
    run_shell_test("ops/ci/tests/release-acceptance.sh");
}
