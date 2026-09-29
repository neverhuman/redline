//! A claim a receipt may back, `durability.<mode>.<failure-model>`, and the
//! checks that decide whether a receipt backs it: its own contents, and the
//! source history between its commit and the one it is checked at.

use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};

use super::{BINARY_INPUTS, CLAIM_MIN_SCENARIOS, EvidenceMode, Receipt, SCHEMA_VERSION};

/// `durability.<mode>.<failure-model>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub text: String,
    pub mode: String,
    pub failure_model: String,
}

impl Claim {
    pub fn parse(text: &str) -> Result<Self> {
        let parts: Vec<&str> = text.split('.').collect();
        let [prefix, mode, failure_model] = parts.as_slice() else {
            bail!("claim {text:?} is not durability.<mode>.<failure-model>");
        };
        if *prefix != "durability" {
            bail!("claim {text:?} does not start with `durability.`");
        }
        if !matches!(*mode, "strict" | "normal" | "unsafe-dev") {
            bail!("claim {text:?} names mode {mode:?}; expected strict, normal or unsafe-dev");
        }
        if !matches!(
            *failure_model,
            "process-kill" | "power-loss" | "media-corruption"
        ) {
            bail!(
                "claim {text:?} names failure model {failure_model:?}; expected process-kill, power-loss or media-corruption"
            );
        }
        Ok(Self {
            text: text.to_owned(),
            mode: (*mode).to_owned(),
            failure_model: (*failure_model).to_owned(),
        })
    }

    pub fn covers(&self, receipt: &Receipt) -> bool {
        receipt.mode.as_str() == self.mode && receipt.failure_model.as_str() == self.failure_model
    }
}

/// Every reason `receipt` cannot back a claim. Empty means it can.
pub fn receipt_problems(receipt: &Receipt) -> Vec<String> {
    let mut problems = Vec::new();
    if receipt.schema_version != SCHEMA_VERSION {
        problems.push(format!(
            "schema_version {} is not {SCHEMA_VERSION}",
            receipt.schema_version
        ));
    }
    if !receipt.passed {
        problems.push("the receipt says passed=false".to_owned());
    }
    for rejection in &receipt.rejections {
        problems.push(format!("the tool rejected the run: {rejection}"));
    }
    let counts = &receipt.scenarios;
    if counts.planned < CLAIM_MIN_SCENARIOS {
        problems.push(format!(
            "{} scenarios planned; a claim needs at least {CLAIM_MIN_SCENARIOS}",
            counts.planned
        ));
    }
    if counts.run != counts.planned || counts.passed != counts.planned || counts.failed != 0 {
        problems.push(format!(
            "scenarios planned {}, run {}, passed {}, failed {}",
            counts.planned, counts.run, counts.passed, counts.failed
        ));
    }
    if counts.faults_observed != counts.run {
        problems.push(format!(
            "the kill landed on a live process in {} of {} runs",
            counts.faults_observed, counts.run
        ));
    }
    if receipt.runs.len() != counts.run {
        problems.push(format!(
            "{} runs listed but scenarios.run is {}",
            receipt.runs.len(),
            counts.run
        ));
    }
    for run in &receipt.runs {
        if !run.passed || !run.verdict.qualified {
            problems.push(format!(
                "scenario {} failed: {}",
                run.index,
                run.verdict.summary()
            ));
        }
        if !run.fault_observed {
            problems.push(format!("scenario {}: no kill landed", run.index));
        }
        if run.acknowledged == 0 {
            problems.push(format!("scenario {}: nothing was acknowledged", run.index));
        }
    }
    if !receipt.failpoint_scan.clean || receipt.failpoint_scan.markers_checked == 0 {
        problems.push(format!(
            "the binary is a failpoint build or was not scanned (found {:?})",
            receipt.failpoint_scan.markers_found
        ));
    }
    if receipt.dirty {
        problems.push("the source tree was dirty".to_owned());
    }
    match receipt.source_sha.as_deref() {
        Some(sha) if is_hex(sha, 40) => {}
        other => problems.push(format!("source_sha {other:?} is not a commit id")),
    }
    if !is_hex(&receipt.binary.sha256, 64) {
        problems.push(format!(
            "binary.sha256 {:?} is not a SHA-256",
            receipt.binary.sha256
        ));
    }
    if receipt.effective_mode.as_deref() != Some(receipt.mode.pragma_value()) {
        problems.push(format!(
            "effective mode {:?} is not the requested {}",
            receipt.effective_mode,
            receipt.mode.as_str()
        ));
    }
    match &receipt.filesystem {
        None => problems.push("the receipt names no filesystem".to_owned()),
        Some(fs) if receipt.mode == EvidenceMode::Strict && fs.is_memory_backed() => {
            problems.push(format!(
                "a Strict receipt on {} ({}) proves nothing about fsync",
                fs.fs_type, fs.mount_point
            ));
        }
        Some(_) => {}
    }
    problems
}

/// Check that `source_sha` is `at` or an ancestor of it, and that no path
/// in [`BINARY_INPUTS`] changed between them.
pub fn source_problems(repo: &Path, source_sha: &str, at: &str) -> Vec<String> {
    let git = |args: &[&str]| Command::new("git").arg("-C").arg(repo).args(args).output();
    let mut problems = Vec::new();
    match git(&["cat-file", "-e", &format!("{source_sha}^{{commit}}")]) {
        Ok(out) if out.status.success() => {}
        Ok(_) => {
            problems.push(format!(
                "source_sha {source_sha} is not a commit in {}",
                repo.display()
            ));
            return problems;
        }
        Err(err) => {
            problems.push(format!("run git in {}: {err}", repo.display()));
            return problems;
        }
    }
    match git(&["merge-base", "--is-ancestor", source_sha, at]) {
        Ok(out) if out.status.success() => {}
        Ok(_) => problems.push(format!(
            "source_sha {source_sha} is not an ancestor of {at}"
        )),
        Err(err) => problems.push(format!("git merge-base: {err}")),
    }
    let mut diff = vec!["diff", "--name-only", source_sha, at, "--"];
    diff.extend(BINARY_INPUTS);
    match git(&diff) {
        Ok(out) if out.status.success() => {
            let changed: Vec<String> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::to_owned)
                .collect();
            if !changed.is_empty() {
                problems.push(format!(
                    "{} binary input files changed between source_sha {source_sha} and {at}, first {:?}",
                    changed.len(),
                    &changed[..changed.len().min(8)]
                ));
            }
        }
        Ok(out) => problems.push(format!(
            "git diff {source_sha} {at}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(err) => problems.push(format!("git diff: {err}")),
    }
    problems
}

fn is_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| b.is_ascii_hexdigit())
}
