//! The durability receipt must fail closed (workplan R10).
//!
//! A receipt that did not pass, a claim with no receipt for its mode and
//! failure model, a failpoint build, and a Strict run on tmpfs must each
//! make the tool exit non-zero. The CLI cases drive the real
//! `redlinedb-bench` binary; none of them needs a release build of the
//! shell, because each is refused before any scenario starts.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use redlinedb_bench::durability_evidence::preflight::{self, FAILPOINT_MARKERS};
use redlinedb_bench::durability_evidence::receipt::{
    BinaryFacts, CLAIM_MIN_SCENARIOS, Claim, EvidenceMode, FailpointScan, FailureModel,
    FilesystemFacts, HostFacts, KillWindow, Receipt, SCHEMA_VERSION, ScenarioCounts, ScenarioRun,
    ToolchainFacts, receipt_problems, source_problems,
};
use redlinedb_bench::durability_evidence::{read_receipt, write_and_check};
use redlinedb_bench::recover::oracle::RecoveryVerdict;

fn bench() -> Command {
    Command::new(env!("CARGO_BIN_EXE_redlinedb-bench"))
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn passing_run(index: usize) -> ScenarioRun {
    ScenarioRun {
        index,
        seed: index as u64,
        kill_after_acks: 16,
        rows_scripted: 4352,
        ready: true,
        acks_at_kill: 16,
        acknowledged: 17,
        recovered_acked: 17,
        child_status: "signal(9)".to_owned(),
        kill_result: "sent SIGKILL".to_owned(),
        fault_observed: true,
        effective_modes: vec!["strict".to_owned(); 3],
        verdict: RecoveryVerdict {
            acknowledged: 17,
            recovered_acked: 17,
            fault_observed: true,
            fault_required: true,
            child_started: true,
            qualified: true,
            ..RecoveryVerdict::default()
        },
        passed: true,
        ..ScenarioRun::default()
    }
}

/// A receipt that backs `durability.strict.process-kill`.
fn good_receipt() -> Receipt {
    let planned = CLAIM_MIN_SCENARIOS;
    Receipt {
        schema_version: SCHEMA_VERSION,
        tool: "redlinedb-bench test durability-evidence".to_owned(),
        created_unix: 1,
        source_sha: Some("a".repeat(40)),
        dirty: false,
        binary: BinaryFacts {
            path: "target/release/redlinedb".to_owned(),
            sha256: "b".repeat(64),
            bytes: 1,
            ..BinaryFacts::default()
        },
        failpoint_scan: FailpointScan {
            clean: true,
            markers_checked: FAILPOINT_MARKERS.len(),
            markers_found: Vec::new(),
        },
        toolchain: ToolchainFacts::default(),
        host: HostFacts::default(),
        mode: EvidenceMode::Strict,
        effective_mode: Some("strict".to_owned()),
        failure_model: FailureModel::ProcessKill,
        filesystem: Some(FilesystemFacts {
            work_dir: "/data/evidence".to_owned(),
            mount_point: "/".to_owned(),
            fs_type: "ext4".to_owned(),
            source: "/dev/nvme0n1p2".to_owned(),
            mount_options: "rw,relatime".to_owned(),
            super_options: "rw".to_owned(),
            device: None,
        }),
        child_environment: vec!["PATH".to_owned()],
        oracle: "recover::oracle".to_owned(),
        seed: 7,
        kill_window: KillWindow {
            min_acks: 8,
            max_acks: 256,
            max_delay_us: 3000,
        },
        scenarios: ScenarioCounts {
            planned,
            run: planned,
            passed: planned,
            failed: 0,
            faults_observed: planned,
        },
        rejections: Vec::new(),
        runs: (0..planned).map(passing_run).collect(),
        passed: true,
        limitations: vec!["SIGKILL is not a power cut".to_owned()],
        logs_dir: None,
    }
}

fn write_receipt(dir: &Path, name: &str, receipt: &Receipt) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, serde_json::to_vec_pretty(receipt).expect("json")).expect("write receipt");
    path
}

fn verify(receipts: &[&Path], claims: &[&str]) -> Output {
    let mut command = bench();
    command.arg("durability-evidence-verify");
    for receipt in receipts {
        command.arg("--receipt").arg(receipt);
    }
    for claim in claims {
        command.args(["--claim", claim]);
    }
    command.output().expect("run durability-evidence-verify")
}

fn problems_mention(receipt: &Receipt, needle: &str) -> bool {
    receipt_problems(receipt)
        .iter()
        .any(|problem| problem.contains(needle))
}

#[test]
fn a_good_receipt_backs_its_claim() {
    let receipt = good_receipt();
    assert_eq!(receipt_problems(&receipt), Vec::<String>::new());
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_receipt(dir.path(), "good.json", &receipt);
    assert_eq!(read_receipt(&path).expect("read back"), receipt);
    let output = verify(&[&path], &["durability.strict.process-kill"]);
    assert!(output.status.success(), "{}", stderr(&output));
}

#[test]
fn a_receipt_that_did_not_pass_exits_non_zero() {
    let mut receipt = good_receipt();
    receipt.passed = false;
    receipt.scenarios.passed -= 1;
    receipt.scenarios.failed = 1;
    receipt.runs[3].passed = false;
    receipt.runs[3].verdict.qualified = false;
    receipt.runs[3].verdict.lost_ack_ids = vec![5];
    assert!(problems_mention(&receipt, "passed=false"));
    assert!(problems_mention(&receipt, "scenario 3 failed"));

    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("written.json");
    let err = write_and_check(&out, &receipt).expect_err("passed=false must fail");
    assert!(err.to_string().contains("passed=false"), "{err:#}");
    assert_eq!(
        read_receipt(&out).expect("the receipt is still written"),
        receipt
    );

    let path = write_receipt(dir.path(), "failed.json", &receipt);
    for claims in [&[][..], &["durability.strict.process-kill"][..]] {
        let output = verify(&[&path], claims);
        assert!(!output.status.success(), "claims {claims:?} accepted");
        assert!(
            stderr(&output).contains("passed=false"),
            "{}",
            stderr(&output)
        );
    }

    // A receipt that says passed=true over a failed run is caught too.
    let mut forged = good_receipt();
    forged.runs[0].verdict.qualified = false;
    assert!(problems_mention(&forged, "scenario 0 failed"));
}

#[test]
fn a_claim_without_a_strict_process_kill_receipt_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut normal = good_receipt();
    normal.mode = EvidenceMode::Normal;
    normal.effective_mode = Some("normal".to_owned());
    let normal_path = write_receipt(dir.path(), "normal.json", &normal);
    assert_eq!(receipt_problems(&normal), Vec::<String>::new());

    let output = verify(&[&normal_path], &["durability.strict.process-kill"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("no receipt is for mode strict and failure model process-kill"),
        "{}",
        stderr(&output)
    );

    // Too few scenarios cannot back a claim either.
    let mut short = good_receipt();
    short.scenarios.planned = CLAIM_MIN_SCENARIOS - 1;
    short.scenarios.run = short.scenarios.planned;
    short.scenarios.passed = short.scenarios.planned;
    short.scenarios.faults_observed = short.scenarios.planned;
    short.runs.truncate(short.scenarios.planned);
    assert!(problems_mention(&short, "a claim needs at least"));
    let short_path = write_receipt(dir.path(), "short.json", &short);
    let output = verify(
        &[&normal_path, &short_path],
        &["durability.strict.process-kill"],
    );
    assert!(!output.status.success());

    // No tool produces a power-loss receipt, so that claim never passes.
    let strict_path = write_receipt(dir.path(), "strict.json", &good_receipt());
    let output = verify(&[&strict_path], &["durability.strict.power-loss"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("power-loss"),
        "{}",
        stderr(&output)
    );

    // An UnsafeDev claim cannot be backed: that mode has no receipt.
    let claim = Claim::parse("durability.unsafe-dev.process-kill").expect("claim");
    assert!(!claim.covers(&good_receipt()));
    for bad in [
        "durability.strict",
        "durability.strict.process-kill.extra",
        "safety.strict.process-kill",
        "durability.fast.process-kill",
        "durability.strict.meteor",
    ] {
        assert!(Claim::parse(bad).is_err(), "{bad} parsed");
    }
}

#[test]
fn a_failpoint_build_is_rejected() {
    // redlinedb-bench links the kernel with `failpoints` on, so it is a
    // real failpoint build.
    let fake = env!("CARGO_BIN_EXE_redlinedb-bench");
    let scan = preflight::scan_binary(Path::new(fake)).expect("scan");
    assert!(!scan.failpoints.clean, "the bench binary scanned clean");
    assert!(
        scan.failpoints
            .markers_found
            .iter()
            .any(|marker| marker == "engine::commit::before_publish"),
        "{:?}",
        scan.failpoints.markers_found
    );

    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("tempdir on disk");
    let out = dir.path().join("receipt.json");
    let output = bench()
        .arg("durability-evidence")
        .arg("--binary")
        .arg(fake)
        .args(["--mode", "normal", "--failure-model", "process-kill"])
        .arg("--out")
        .arg(&out)
        .arg("--work-dir")
        .arg(dir.path().join("work"))
        .arg("--repo")
        .arg(workspace_root())
        .output()
        .expect("run durability-evidence");
    assert!(!output.status.success(), "a failpoint build was accepted");
    let receipt = read_receipt(&out).expect("the refusal still writes a receipt");
    assert!(!receipt.passed);
    assert!(!receipt.failpoint_scan.clean);
    assert_eq!(
        receipt.scenarios.run, 0,
        "scenarios ran on a failpoint build"
    );
    assert!(
        receipt
            .rejections
            .iter()
            .any(|reason| reason.contains("failpoint build")),
        "{:?}",
        receipt.rejections
    );
    assert!(problems_mention(&receipt, "failpoint build"));

    let mut forged = good_receipt();
    forged.failpoint_scan.clean = false;
    forged.failpoint_scan.markers_found = vec!["wal::write_encoded".to_owned()];
    assert!(problems_mention(&forged, "failpoint build"));
    let mut unscanned = good_receipt();
    unscanned.failpoint_scan.markers_checked = 0;
    assert!(problems_mention(&unscanned, "failpoint build"));
}

/// A stand-in binary with no failpoint strings, for refusals that happen
/// before any scenario runs.
fn clean_stand_in(dir: &Path) -> PathBuf {
    let path = dir.join("redlinedb");
    fs::write(&path, "#!/bin/sh\necho 'redlinedb stand-in'\n").expect("write stand-in");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    path
}

fn tmpfs_dir() -> Option<PathBuf> {
    let mountinfo = fs::read_to_string("/proc/self/mountinfo").ok()?;
    let shm = Path::new("/dev/shm");
    let entry = preflight::mount_for(&mountinfo, &shm.canonicalize().ok()?)?;
    (entry.fs_type == "tmpfs").then(|| shm.to_path_buf())
}

#[test]
fn strict_on_tmpfs_is_rejected() {
    let mut receipt = good_receipt();
    let fs = receipt.filesystem.as_mut().expect("filesystem");
    fs.fs_type = "tmpfs".to_owned();
    fs.mount_point = "/dev/shm".to_owned();
    assert!(problems_mention(&receipt, "proves nothing about fsync"));
    receipt.filesystem.as_mut().expect("filesystem").fs_type = "ramfs".to_owned();
    assert!(problems_mention(&receipt, "proves nothing about fsync"));
    // Normal never waits for fsync, so tmpfs does not disqualify it.
    receipt.mode = EvidenceMode::Normal;
    receipt.effective_mode = Some("normal".to_owned());
    assert_eq!(receipt_problems(&receipt), Vec::<String>::new());
    let mut unnamed = good_receipt();
    unnamed.filesystem = None;
    assert!(problems_mention(&unnamed, "names no filesystem"));

    // End to end, on this host's tmpfs. Linux CI hosts mount /dev/shm as
    // tmpfs; the unit checks above hold everywhere.
    let Some(shm) = tmpfs_dir() else {
        eprintln!("/dev/shm is not tmpfs here; end-to-end tmpfs refusal not exercised");
        return;
    };
    let dir = tempfile::tempdir_in(&shm).expect("tempdir on tmpfs");
    let out = dir.path().join("receipt.json");
    let output = bench()
        .arg("durability-evidence")
        .arg("--binary")
        .arg(clean_stand_in(dir.path()))
        .args(["--mode", "strict", "--failure-model", "process-kill"])
        .arg("--out")
        .arg(&out)
        .arg("--work-dir")
        .arg(dir.path().join("work"))
        .arg("--repo")
        .arg(workspace_root())
        .output()
        .expect("run durability-evidence");
    assert!(!output.status.success(), "Strict on tmpfs was accepted");
    let receipt = read_receipt(&out).expect("receipt");
    assert!(!receipt.passed);
    assert!(receipt.failpoint_scan.clean, "{:?}", receipt.failpoint_scan);
    assert_eq!(receipt.scenarios.run, 0);
    assert_eq!(
        receipt.filesystem.as_ref().map(|fs| fs.fs_type.as_str()),
        Some("tmpfs")
    );
    assert!(
        receipt
            .rejections
            .iter()
            .any(|reason| reason.contains("fsync does nothing")),
        "{:?}",
        receipt.rejections
    );
}

#[test]
fn other_receipt_defects_disqualify() {
    let mut dirty = good_receipt();
    dirty.dirty = true;
    assert!(problems_mention(&dirty, "dirty"));
    let mut no_sha = good_receipt();
    no_sha.source_sha = None;
    assert!(problems_mention(&no_sha, "not a commit id"));
    let mut wrong_mode = good_receipt();
    wrong_mode.effective_mode = Some("normal".to_owned());
    assert!(problems_mention(&wrong_mode, "effective mode"));
    let mut no_fault = good_receipt();
    no_fault.runs[2].fault_observed = false;
    no_fault.scenarios.faults_observed -= 1;
    assert!(problems_mention(&no_fault, "no kill landed"));
    let mut nothing_acked = good_receipt();
    nothing_acked.runs[1].acknowledged = 0;
    assert!(problems_mention(&nothing_acked, "nothing was acknowledged"));
    let mut missing_runs = good_receipt();
    missing_runs.runs.pop();
    assert!(problems_mention(&missing_runs, "runs listed"));
    let mut old_schema = good_receipt();
    old_schema.schema_version = SCHEMA_VERSION + 1;
    assert!(problems_mention(&old_schema, "schema_version"));
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "-c",
            "user.name=receipt test",
            "-c",
            "user.email=receipt@test.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("run git");
    assert!(output.status.success(), "git {args:?}: {}", stderr(&output));
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn commit_file(repo: &Path, path: &str, text: &str) -> String {
    let file = repo.join(path);
    fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
    fs::write(&file, text).expect("write");
    git(repo, &["add", path]);
    git(repo, &["commit", "-q", "-m", path]);
    git(repo, &["rev-parse", "HEAD"])
}

#[test]
fn a_receipt_stays_valid_only_while_the_binary_inputs_do_not_change() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = dir.path();
    git(repo, &["init", "-q"]);
    let receipt_sha = commit_file(repo, "crates/kernel/src/lib.rs", "// v1\n");
    let docs_sha = commit_file(repo, "docs/manual/durability.md", "claim\n");
    assert_eq!(
        source_problems(repo, &receipt_sha, &docs_sha),
        Vec::<String>::new()
    );
    let code_sha = commit_file(repo, "crates/kernel/src/lib.rs", "// v2\n");
    let problems = source_problems(repo, &receipt_sha, &code_sha);
    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("binary input files changed")),
        "{problems:?}"
    );
    // A later receipt is not evidence for an earlier release.
    let problems = source_problems(repo, &code_sha, &docs_sha);
    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("not an ancestor")),
        "{problems:?}"
    );
    let problems = source_problems(repo, &"0".repeat(40), &docs_sha);
    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("not a commit")),
        "{problems:?}"
    );
}

/// Every failpoint name in the kernel must be in the scan list, or a build
/// with only that failpoint would pass the scan.
#[test]
fn the_failpoint_scan_knows_every_kernel_failpoint() {
    let src = workspace_root().join("crates/kernel/src");
    let mut found = std::collections::BTreeSet::new();
    let mut stack = vec![src];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read_dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            let text = fs::read_to_string(&path).expect("read source");
            for line in text.lines() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                for call in ["fail_point!(\"", "io_error_under(\""] {
                    let mut rest = code;
                    while let Some(at) = rest.find(call) {
                        let tail = &rest[at + call.len()..];
                        let end = tail.find('"').expect("closing quote");
                        found.insert(tail[..end].to_owned());
                        rest = &tail[end..];
                    }
                }
            }
        }
    }
    assert!(found.len() > 20, "found only {found:?}");
    let missing: Vec<&String> = found
        .iter()
        .filter(|name| !FAILPOINT_MARKERS.contains(&name.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "failpoints the scan misses: {missing:?}"
    );
    let stale: Vec<&&str> = FAILPOINT_MARKERS
        .iter()
        .filter(|marker| marker.contains("::") && !found.contains(**marker))
        .collect();
    assert!(
        stale.is_empty(),
        "scan markers no kernel failpoint uses: {stale:?}"
    );
}
