//! `durability-evidence-verify`: receipts read back and held to the claim
//! tags the docs carry.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;

use super::receipt::{self, Claim, Receipt};

#[derive(Debug, Clone, Args)]
pub struct DurabilityEvidenceVerifyArgs {
    /// A receipt to consider; repeat for several.
    #[arg(long = "receipt", required = true)]
    pub receipts: Vec<PathBuf>,
    /// `durability.<mode>.<failure-model>`; each needs one receipt that
    /// covers it and passes every check. Without a claim, every receipt
    /// must pass.
    #[arg(long = "claim")]
    pub claims: Vec<String>,
    /// Check that each receipt's source_sha is `--at` or an ancestor of it
    /// with no change to the binary's inputs in between.
    #[arg(long)]
    pub repo: Option<PathBuf>,
    #[arg(long, default_value = "HEAD", requires = "repo")]
    pub at: String,
}

pub fn read_receipt(path: &Path) -> Result<Receipt> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse receipt {}", path.display()))
}

/// Every claim needs one receipt that covers it and has no problem;
/// without claims, every receipt must be free of problems.
pub fn verify(args: &DurabilityEvidenceVerifyArgs) -> Result<()> {
    let mut receipts = Vec::new();
    for path in &args.receipts {
        receipts.push((path.clone(), read_receipt(path)?));
    }
    let problems_of = |receipt: &Receipt| {
        let mut problems = receipt::receipt_problems(receipt);
        if let (Some(repo), Some(sha)) = (&args.repo, &receipt.source_sha) {
            problems.extend(receipt::source_problems(repo, sha, &args.at));
        }
        problems
    };
    let mut failures = Vec::new();
    if args.claims.is_empty() {
        for (path, receipt) in &receipts {
            for problem in problems_of(receipt) {
                failures.push(format!("{}: {problem}", path.display()));
            }
        }
    }
    for text in &args.claims {
        let claim = Claim::parse(text)?;
        let covering: Vec<&(PathBuf, Receipt)> = receipts
            .iter()
            .filter(|(_, receipt)| claim.covers(receipt))
            .collect();
        if covering.is_empty() {
            failures.push(format!(
                "{}: no receipt is for mode {} and failure model {}",
                claim.text, claim.mode, claim.failure_model
            ));
            continue;
        }
        let mut reasons = Vec::new();
        let mut satisfied = false;
        for (path, receipt) in covering {
            let problems = problems_of(receipt);
            if problems.is_empty() {
                satisfied = true;
                eprintln!(
                    "durability-evidence-verify: {} backed by {}",
                    claim.text,
                    path.display()
                );
                break;
            }
            reasons.extend(
                problems
                    .into_iter()
                    .map(|problem| format!("{}: {problem}", path.display())),
            );
        }
        if !satisfied {
            failures.push(format!(
                "{}: no covering receipt passes:\n  {}",
                claim.text,
                reasons.join("\n  ")
            ));
        }
    }
    if failures.is_empty() {
        return Ok(());
    }
    bail!(
        "durability-evidence-verify failed:\n{}",
        failures.join("\n")
    )
}
