//! `build-contract`: what exactly a perf run measured (BM3-05).
//!
//! A timing is only interpretable next to the binaries that produced it.
//! The contract records, at measurement time: the Rust toolchain
//! (`rustc -vV`); the target, the SQLite reference and the runner, each by
//! path, SHA-256, size and `--version`; the reference's
//! `PRAGMA compile_options`; and how the target was built, as its builder
//! declared it (cargo profile, features and the effective RUSTFLAGS).
//! Nothing about the build is guessed: a binary whose builder declared
//! nothing is recorded with `declared: false` and null fields, and a
//! non-PGO build records no training corpus.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::{HostMetadata, capture_w2_runtime_metadata, sha256_file};

pub const BUILD_CONTRACT_SCHEMA: &str = "redline-perf-build-contract-v1";

/// How the target was built, as its builder declared it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DeclaredBuild {
    /// False when the builder declared nothing; every field is then null.
    pub declared: bool,
    pub profile: Option<String>,
    pub features: Option<String>,
    /// The RUSTFLAGS the build ran with (CARGO_ENCODED_RUSTFLAGS cleared),
    /// which cargo uses instead of any config rustflags. `""` is a declared
    /// build with no flags; null is undeclared.
    pub rustflags: Option<String>,
}

#[derive(Clone, Debug)]
pub struct BuildContractInput {
    pub output: PathBuf,
    pub target_bin: PathBuf,
    pub reference_bin: PathBuf,
    pub runner_bin: PathBuf,
    pub build: DeclaredBuild,
    pub pgo_training_corpus: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BuildContract {
    pub schema_version: &'static str,
    pub captured_at_utc: String,
    pub host: HostMetadata,
    pub toolchain: Toolchain,
    pub target: TargetBinary,
    pub reference: ReferenceBinary,
    pub runner: BinaryIdentity,
    pub optimization: Optimization,
}

#[derive(Clone, Debug, Serialize)]
pub struct Toolchain {
    /// `rustc -vV`, one entry per line.
    pub rustc_verbose_version: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BinaryIdentity {
    /// The canonical path: absolute, with symlinks resolved.
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub version: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct TargetBinary {
    #[serde(flatten)]
    pub identity: BinaryIdentity,
    pub build: DeclaredBuild,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReferenceBinary {
    #[serde(flatten)]
    pub identity: BinaryIdentity,
    /// `PRAGMA compile_options` of the reference shell, one per option.
    pub compile_options: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Optimization {
    /// The workload a PGO profile was trained on; null without PGO.
    pub pgo_training_corpus: Option<String>,
}

pub fn write_build_contract(input: &BuildContractInput) -> Result<BuildContract> {
    let contract = capture_build_contract(input)?;
    if let Some(parent) = input.output.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create build contract directory {}", parent.display()))?;
    }
    fs::write(
        &input.output,
        format!("{}\n", serde_json::to_string_pretty(&contract)?),
    )
    .with_context(|| format!("write build contract {}", input.output.display()))?;
    Ok(contract)
}

pub fn capture_build_contract(input: &BuildContractInput) -> Result<BuildContract> {
    let (captured_at_utc, _, host) = capture_w2_runtime_metadata()?;
    let build = &input.build;
    if !build.declared
        && (build.profile.is_some() || build.features.is_some() || build.rustflags.is_some())
    {
        bail!("an undeclared build cannot carry build fields");
    }
    Ok(BuildContract {
        schema_version: BUILD_CONTRACT_SCHEMA,
        captured_at_utc,
        host,
        toolchain: Toolchain {
            rustc_verbose_version: output_lines("rustc", &["-vV"], Path::new("rustc"))?,
        },
        target: TargetBinary {
            identity: binary_identity(&input.target_bin)?,
            build: build.clone(),
        },
        reference: ReferenceBinary {
            identity: binary_identity(&input.reference_bin)?,
            compile_options: output_lines(
                &input.reference_bin.display().to_string(),
                &[":memory:", "PRAGMA compile_options;"],
                &input.reference_bin,
            )?,
        },
        runner: binary_identity(&input.runner_bin)?,
        optimization: Optimization {
            pgo_training_corpus: input.pgo_training_corpus.clone(),
        },
    })
}

fn binary_identity(path: &Path) -> Result<BinaryIdentity> {
    let path = &fs::canonicalize(path).with_context(|| format!("resolve {}", path.display()))?;
    let size_bytes = path
        .metadata()
        .with_context(|| format!("stat {}", path.display()))?
        .len();
    let version = output_lines(&path.display().to_string(), &["--version"], path)?.join("\n");
    Ok(BinaryIdentity {
        path: path.display().to_string(),
        sha256: sha256_file(path)?,
        size_bytes,
        version,
    })
}

/// The non-empty lines a successful run of `program` printed.
fn output_lines(program: &str, arguments: &[&str], label: &Path) -> Result<Vec<String>> {
    let output = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("run {} {}", label.display(), arguments.join(" ")))?;
    if !output.status.success() {
        bail!(
            "{} {} exited with {}: {}",
            label.display(),
            arguments.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let lines = String::from_utf8(output.stdout)
        .with_context(|| format!("{} printed non-UTF-8 output", label.display()))?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        bail!(
            "{} {} printed nothing",
            label.display(),
            arguments.join(" ")
        );
    }
    Ok(lines)
}

#[cfg(all(test, unix))]
#[path = "build_contract_tests.rs"]
mod tests;
