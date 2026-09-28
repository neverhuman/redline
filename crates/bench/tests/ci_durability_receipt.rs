//! Release acceptance binds a `durability-evidence` artifact
//! (ops/release/acceptance-receipts). ci.yml's durability-receipt job makes
//! it on a release run: it verifies the durability receipts committed under
//! benchmark-results/durability/ against the claim at the tag's commit with
//! ops/ci/durability-receipt.sh and uploads them. Receipts are taken once per
//! release candidate, so the job runs only when ci.yml is called with a tag,
//! and RedlineDB/required accepts its skip only when there is no tag.
//!
//! These checks read the workflow text; ops/ci/tests/durability-receipt.sh
//! exercises the script.

#[path = "support/workflow_text.rs"]
mod workflow_text;
use workflow_text::{job, read, run_shell_test, steps_using};

#[test]
fn the_release_run_verifies_and_uploads_the_committed_receipts() {
    let ci = read(".github/workflows/ci.yml");
    let body = job(&ci, "durability-receipt");
    assert!(
        body.contains("    if: inputs.tag != ''\n"),
        "durability-receipt must run on release (tag) runs only"
    );
    let checkout = &steps_using(&body, "actions/checkout@")[0];
    assert!(
        checkout.contains("fetch-depth: 0"),
        "the verifier needs the history between each receipt's commit and the tag"
    );
    let verify = body
        .find("run: bash ops/ci/durability-receipt.sh target/durability-evidence")
        .expect("durability-receipt runs ops/ci/durability-receipt.sh");
    let uploads = steps_using(&body, "actions/upload-artifact@");
    assert_eq!(uploads.len(), 1, "one upload: {uploads:?}");
    let upload = &uploads[0];
    assert!(
        upload.contains("name: durability-evidence,")
            && upload.contains("path: target/durability-evidence,")
            && upload.contains("if-no-files-found: error"),
        "the upload must be the durability-evidence artifact and fail without files: {upload}"
    );
    assert!(
        !upload.contains("if: always()"),
        "a failed verification must not upload anything"
    );
    assert!(
        verify < body.find("actions/upload-artifact@").unwrap(),
        "verify before upload"
    );
    let receipts = read("ops/release/acceptance-receipts");
    assert!(
        receipts
            .lines()
            .any(|line| line.starts_with("durability-evidence ")
                && line.contains("durability-receipt")),
        "ops/release/acceptance-receipts must name the job that uploads durability-evidence"
    );
}

#[test]
fn required_accepts_the_skip_only_without_a_tag() {
    let ci = read(".github/workflows/ci.yml");
    let required = job(&ci, "required");
    assert!(required.contains("          RELEASE_TAG: ${{ inputs.tag || '' }}\n"));
    assert!(
        required.contains(
            r#"all(.value.result == "success" or ($tag == "" and .key == "durability-receipt" and .value.result == "skipped"))"#
        ),
        "RedlineDB/required must require success from every job, durability-receipt too on a tag run"
    );
}

#[test]
fn the_receipt_script_fails_closed() {
    run_shell_test("ops/ci/tests/durability-receipt.sh");
}
