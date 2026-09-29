//! A run: every selected workload, repeated, for each engine, each case in
//! a child process, the engines alternating which goes first.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::case::Measured;
use crate::image::{self, ImageKind};
use crate::pair::{Engine, Pair};
use crate::workloads::{self, CATALOG, Workload};

/// A repetition longer than this lowers the case's repetitions to three.
const LONG_REP: Duration = Duration::from_secs(10);

pub struct RunArgs {
    pub label: String,
    pub run: u32,
    pub rows: u64,
    pub pair: Pair,
    pub reps: u32,
    pub divisor: u64,
    pub engines: Vec<Engine>,
    pub workloads: Option<Vec<String>>,
    pub image_store: PathBuf,
    pub work_root: PathBuf,
    pub out: PathBuf,
    pub budget: Duration,
    pub image_budget: Duration,
}

/// The workloads a run measures: the requested ones, or the catalog. The
/// strict pair measures only writers; its reads would repeat the normal
/// pair's.
pub fn selected(requested: &Option<Vec<String>>, pair: Pair) -> Result<Vec<Workload>> {
    let mut chosen = Vec::new();
    match requested {
        Some(ids) => {
            for id in ids {
                chosen.push(workloads::find(id)?);
            }
        }
        None => chosen.extend(CATALOG.iter().copied()),
    }
    if pair == Pair::Strict {
        chosen.retain(|workload| workload.writes);
    }
    Ok(chosen)
}

pub fn run(args: &RunArgs) -> Result<()> {
    let exe = std::env::current_exe().context("locate this binary")?;
    let chosen = selected(&args.workloads, args.pair)?;
    if chosen.is_empty() {
        bail!("no workload selected");
    }
    fs::create_dir_all(&args.image_store)?;
    fs::create_dir_all(&args.work_root)?;

    let mut images = BTreeMap::new();
    for engine in &args.engines {
        for kind in [ImageKind::Base, ImageKind::AfterUpdates] {
            if kind == ImageKind::AfterUpdates
                && !chosen.iter().any(|workload| workload.image == kind)
            {
                continue;
            }
            let dir = ensure_image(&exe, args, *engine, kind)?;
            let sha = image::tree_sha256(&dir)?;
            images.insert((*engine, kind), (dir, sha));
        }
    }

    let mut out = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.out)
        .with_context(|| format!("open {}", args.out.display()))?;
    let mut seq = 0u64;
    for workload in &chosen {
        let mut reps = args.reps;
        let mut rep = 0;
        while rep < reps {
            let mut order = args.engines.clone();
            if rep % 2 == 1 {
                order.reverse();
            }
            for engine in order {
                let (dir, sha) = &images[&(engine, workload.image)];
                let work = args
                    .work_root
                    .join(format!("case-{}-{seq}", std::process::id()));
                let started = Instant::now();
                let outcome = run_child(&exe, args, engine, workload, dir, &work);
                let wall = started.elapsed();
                let _ = fs::remove_dir_all(&work);
                let line = record_line(args, engine, workload, rep, seq, sha, outcome, wall)?;
                writeln!(out, "{line}")?;
                out.flush()?;
                if rep == 0 && wall > LONG_REP {
                    reps = reps.min(3);
                }
                seq += 1;
            }
            rep += 1;
        }
    }

    // An image a case changed would make every later case measure
    // something else; refuse the run.
    for ((engine, kind), (dir, sha)) in &images {
        let now = image::tree_sha256(dir)?;
        if &now != sha {
            bail!(
                "image {engine}/{kind:?} at {} changed during the run",
                dir.display()
            );
        }
    }
    Ok(())
}

/// The store directory of an engine's image, built by a child if missing.
fn ensure_image(exe: &Path, args: &RunArgs, engine: Engine, kind: ImageKind) -> Result<PathBuf> {
    let owner = match engine {
        Engine::Redline => format!("redline-{}", args.label),
        Engine::Sqlite => format!("sqlite-{}", rusqlite::version()),
    };
    let kind_name = match kind {
        ImageKind::Base => "base",
        ImageKind::AfterUpdates => "after-updates",
    };
    let dir = args
        .image_store
        .join(format!("{owner}-{}-{kind_name}", args.rows));
    if dir.join(".complete").exists() {
        return Ok(dir);
    }
    let _ = fs::remove_dir_all(&dir);
    let mut command = Command::new(exe);
    command.args([
        "image",
        "--engine",
        &engine.to_string(),
        "--kind",
        kind_name,
        "--rows",
        &args.rows.to_string(),
        "--out",
    ]);
    command.arg(&dir);
    if kind == ImageKind::AfterUpdates {
        command
            .arg("--base")
            .arg(ensure_image(exe, args, engine, ImageKind::Base)?);
    }
    let status = wait_with_budget(command.stdout(Stdio::null()), args.image_budget)?;
    match status {
        Some(0) => {
            fs::write(dir.join(".complete"), b"")?;
            Ok(dir)
        }
        Some(code) => bail!("building image {} failed with exit {code}", dir.display()),
        None => bail!(
            "building image {} took longer than {:?}",
            dir.display(),
            args.image_budget
        ),
    }
}

enum Outcome {
    Measured(Box<Measured>),
    Failed(String),
    TimedOut,
}

fn run_child(
    exe: &Path,
    args: &RunArgs,
    engine: Engine,
    workload: &Workload,
    image_dir: &Path,
    work: &Path,
) -> Outcome {
    let mut command = Command::new(exe);
    command
        .args([
            "case",
            "--engine",
            &engine.to_string(),
            "--workload",
            workload.id,
            "--rows",
            &args.rows.to_string(),
            "--pair",
            &args.pair.to_string(),
            "--work-divisor",
            &args.divisor.to_string(),
            "--image",
        ])
        .arg(image_dir)
        .arg("--work")
        .arg(work)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => return Outcome::Failed(format!("spawn: {err}")),
    };
    // Drain both pipes while the child runs, so a large stderr cannot fill
    // a pipe and stall the child until it is killed as timed out.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_string(&mut text);
            }
            text
        })
    };
    let stdout_reader = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let stderr_reader = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let deadline = Instant::now() + args.budget;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Outcome::TimedOut;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(err) => return Outcome::Failed(format!("wait: {err}")),
        }
    };
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    if !status.success() {
        return Outcome::Failed(format!("exit {status}: {}", stderr.trim()));
    }
    match serde_json::from_str::<Measured>(stdout.trim()) {
        Ok(measured) => Outcome::Measured(Box::new(measured)),
        Err(err) => Outcome::Failed(format!("unreadable record: {err}")),
    }
}

/// Wait for `command`, killing it after `budget`. `None` means it was killed.
fn wait_with_budget(command: &mut Command, budget: Duration) -> Result<Option<i32>> {
    let mut child = command.spawn()?;
    let deadline = Instant::now() + budget;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status.code().unwrap_or(-1)));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[allow(clippy::too_many_arguments)]
fn record_line(
    args: &RunArgs,
    engine: Engine,
    workload: &Workload,
    rep: u32,
    seq: u64,
    image_sha256: &str,
    outcome: Outcome,
    wall: Duration,
) -> Result<String> {
    let label = match engine {
        Engine::Redline => args.label.clone(),
        Engine::Sqlite => "sqlite".to_owned(),
    };
    let mut line = match outcome {
        Outcome::Measured(measured) => {
            let mut value = serde_json::to_value(*measured)?;
            value["status"] = serde_json::json!("ok");
            value
        }
        Outcome::Failed(error) => serde_json::json!({
            "schema": crate::case::RECORD_SCHEMA,
            "engine": engine.to_string(),
            "pair": args.pair.to_string(),
            "rows": args.rows,
            "workload": workload.id,
            "unit": workload.unit,
            "status": "error",
            "error": error,
        }),
        Outcome::TimedOut => serde_json::json!({
            "schema": crate::case::RECORD_SCHEMA,
            "engine": engine.to_string(),
            "pair": args.pair.to_string(),
            "rows": args.rows,
            "workload": workload.id,
            "unit": workload.unit,
            "status": "timeout",
            "budget_ns": args.budget.as_nanos() as u64,
        }),
    };
    line["label"] = serde_json::json!(label);
    // The version whose run measured this record; SQLite is measured
    // beside each version and compared with that version only.
    line["with"] = serde_json::json!(args.label);
    line["run"] = serde_json::json!(args.run);
    line["rep"] = serde_json::json!(rep);
    line["seq"] = serde_json::json!(seq);
    line["wall_ns"] = serde_json::json!(wall.as_nanos() as u64);
    line["image_sha256"] = serde_json::json!(image_sha256);
    Ok(serde_json::to_string(&line)?)
}
