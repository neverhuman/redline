//! Strict runs share a disk phase: each repetition puts the versions'
//! SQLite controls next to each other, between the two engine cases.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use clap::Args as ClapArgs;

use crate::image::{self, ImageKind};
use crate::pair::{Engine, Pair};
use crate::run::{self, RunArgs};

pub(crate) const METHOD: &str = "paired-repetition-v1";
pub(crate) const MIN_REPS: u32 = 7;

#[derive(ClapArgs)]
pub(crate) struct Args {
    /// One or two LABEL=BINARY entries, built with the same harness/SQLite.
    #[arg(long, required = true)]
    pub version: Vec<String>,
    #[arg(long, default_value_t = 1)]
    pub run: u32,
    #[arg(long, default_value_t = 20_000)]
    pub rows: u64,
    /// Always completed, even when a case takes more than ten seconds.
    #[arg(long, default_value_t = MIN_REPS)]
    pub reps: u32,
    #[arg(long)]
    pub image_store: PathBuf,
    #[arg(long)]
    pub work_root: PathBuf,
    /// Records go in OUT/LABEL/run-N-strict.jsonl.
    #[arg(long)]
    pub out: PathBuf,
    #[arg(long, value_delimiter = ',')]
    pub workloads: Option<Vec<String>>,
    #[arg(long, default_value_t = 600)]
    pub budget_s: u64,
    #[arg(long, default_value_t = 3_600)]
    pub image_budget_s: u64,
}

/// Rotate which version surrounds the controls. For two versions this is
/// engine A, SQLite A, SQLite B, engine B; the next repetition swaps A/B.
pub(crate) fn order(versions: usize, run: u32, rep: u32) -> Vec<(usize, Engine)> {
    let first = ((u64::from(run) - 1 + u64::from(rep)) % versions as u64) as usize;
    if versions == 1 {
        return if rep.is_multiple_of(2) {
            vec![(0, Engine::Redline), (0, Engine::Sqlite)]
        } else {
            vec![(0, Engine::Sqlite), (0, Engine::Redline)]
        };
    }
    let second = 1 - first;
    vec![
        (first, Engine::Redline),
        (first, Engine::Sqlite),
        (second, Engine::Sqlite),
        (second, Engine::Redline),
    ]
}

pub(crate) fn run(args: &Args) -> Result<()> {
    if !(1..=2).contains(&args.version.len())
        || args.reps < MIN_REPS
        || args.run == 0
        || args.rows < 10
        || args.budget_s == 0
        || args.image_budget_s == 0
    {
        bail!(
            "Strict needs one or two versions, at least {MIN_REPS} repetitions, a positive run/budget and at least ten rows"
        );
    }
    let chosen = run::selected(&args.workloads, Pair::Strict)?;
    if chosen.is_empty() {
        bail!("no Strict write workload selected");
    }
    fs::create_dir_all(&args.image_store)?;
    fs::create_dir_all(&args.work_root)?;
    let mut labels = BTreeSet::new();
    let mut versions = Vec::new();
    let mut images = BTreeMap::new();
    let mut outputs = Vec::new();
    for (index, spec) in args.version.iter().enumerate() {
        let Some((label, binary)) = spec.split_once('=') else {
            bail!("a Strict version must be LABEL=BINARY");
        };
        if label.is_empty()
            || label == "sqlite"
            || !label.as_bytes()[0].is_ascii_alphanumeric()
            || !label
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._+-".contains(&c))
            || !labels.insert(label)
        {
            bail!("bad or repeated Strict version label {label}");
        }
        let binary = PathBuf::from(binary);
        if !binary.is_file() {
            bail!("Strict version binary {} is missing", binary.display());
        }
        let output = args
            .out
            .join(label)
            .join(format!("run-{}-strict.jsonl", args.run));
        fs::create_dir_all(output.parent().expect("label directory"))?;
        let run_args = RunArgs {
            label: label.to_owned(),
            run: args.run,
            rows: args.rows,
            pair: Pair::Strict,
            reps: args.reps,
            divisor: 1,
            engines: vec![Engine::Redline, Engine::Sqlite],
            workloads: args.workloads.clone(),
            image_store: args.image_store.clone(),
            work_root: args.work_root.clone(),
            out: output.clone(),
            budget: Duration::from_secs(args.budget_s),
            image_budget: Duration::from_secs(args.image_budget_s),
        };
        for engine in [Engine::Redline, Engine::Sqlite] {
            for kind in [ImageKind::Base, ImageKind::AfterUpdates] {
                if kind == ImageKind::AfterUpdates && !chosen.iter().any(|w| w.image == kind) {
                    continue;
                }
                let dir = run::ensure_image(&binary, &run_args, engine, kind)?;
                let sha = image::tree_sha256(&dir)?;
                images.insert((index, engine, kind), (dir, sha));
            }
        }
        outputs.push(
            fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(output)?,
        );
        versions.push((binary, run_args));
    }
    let mut seq = 0;
    for workload in &chosen {
        for rep in 0..args.reps {
            for (index, engine) in order(versions.len(), args.run, rep) {
                let (binary, run_args) = &versions[index];
                let (dir, sha) = &images[&(index, engine, workload.image)];
                let work = args
                    .work_root
                    .join(format!("strict-case-{}-{seq}", std::process::id()));
                let start = Instant::now();
                let outcome = run::run_child(binary, run_args, engine, workload, dir, &work);
                let wall = start.elapsed();
                let _ = fs::remove_dir_all(&work);
                let line =
                    run::record_line(run_args, engine, workload, rep, seq, sha, outcome, wall)?;
                writeln!(outputs[index], "{line}")?;
                outputs[index].flush()?;
                seq += 1;
            }
        }
    }
    for (_, (dir, sha)) in images {
        if image::tree_sha256(&dir)? != sha {
            bail!("Strict image {} changed during the run", dir.display());
        }
    }
    Ok(())
}
