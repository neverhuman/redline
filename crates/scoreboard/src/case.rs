//! One measurement: one engine, one workload, one repetition, in a process
//! of its own, on a fresh copy of the image. The record goes to stdout as
//! one JSON line.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::driver::Driver;
use crate::image::{self, ImageKind};
use crate::measure::{self, Latency};
use crate::pair::{Engine, Pair};
use crate::redline::Redline;
use crate::sqlite::Sqlite;
use crate::workloads::{self, Workload};

pub const RECORD_SCHEMA: &str = "redline-scoreboard-record-v1";

/// What a case measured. The parent adds the label, run and repetition.
#[derive(Debug, Serialize, serde::Deserialize)]
pub struct Measured {
    pub schema: String,
    pub engine: String,
    pub engine_version: String,
    pub pair: String,
    pub rows: u64,
    pub workload: String,
    pub unit: String,
    /// The catalog's work divided by this; only 1 may be published.
    pub work_divisor: u64,
    pub ops: u64,
    pub elapsed_ns: u64,
    pub cpu_user_ns: u64,
    pub cpu_sys_ns: u64,
    pub rss_peak_kib: u64,
    pub io: serde_json::Value,
    pub latency_ns: Option<serde_json::Value>,
    pub counters: Option<serde_json::Value>,
    pub digest: i64,
    pub settings: serde_json::Value,
}

/// The file or directory an engine keeps its database in, inside `dir`.
pub fn db_path(dir: &Path, engine: Engine) -> PathBuf {
    match engine {
        Engine::Redline => dir.join("db.redline"),
        Engine::Sqlite => dir.join("db.sqlite"),
    }
}

/// Build an image of `kind` in the empty directory `out`. `after-updates`
/// starts from a copy of the base image at `base`.
pub fn build_image(
    engine: Engine,
    kind: ImageKind,
    rows: u64,
    out: &Path,
    base: Option<&Path>,
) -> Result<()> {
    match kind {
        ImageKind::Base => {
            std::fs::create_dir_all(out)?;
            let path = db_path(out, engine);
            match engine {
                Engine::Redline => image::build(&Redline::open(&path, Pair::Normal)?, rows),
                Engine::Sqlite => image::build(&Sqlite::open(&path, Pair::Normal)?, rows),
            }
        }
        ImageKind::AfterUpdates => {
            let base = base.context("an after-updates image needs --base")?;
            image::copy_dir(base, out)?;
            let path = db_path(out, engine);
            match engine {
                Engine::Redline => image::apply_updates(Redline::open(&path, Pair::Normal)?, rows),
                Engine::Sqlite => {
                    let sqlite = Sqlite::open(&path, Pair::Normal)?;
                    sqlite.keep_log()?;
                    image::apply_updates(sqlite, rows)
                }
            }
        }
    }
}

/// Copy `image` to `work`, then measure `workload` there.
pub fn run_case(
    engine: Engine,
    workload: &Workload,
    rows: u64,
    pair: Pair,
    divisor: u64,
    image_dir: &Path,
    work: &Path,
) -> Result<Measured> {
    image::copy_dir(image_dir, work)?;
    let path = db_path(work, engine);
    let shape = Shape {
        workload,
        rows,
        pair,
        divisor,
    };
    match engine {
        Engine::Redline => measure_with(|| Redline::open(&path, pair), &shape),
        Engine::Sqlite => measure_with(|| Sqlite::open(&path, pair), &shape),
    }
}

/// What one case measures.
struct Shape<'a> {
    workload: &'a Workload,
    rows: u64,
    pair: Pair,
    divisor: u64,
}

fn measure_with<D: Driver>(open: impl Fn() -> Result<D>, shape: &Shape<'_>) -> Result<Measured> {
    let Shape {
        workload,
        rows,
        pair,
        divisor,
    } = *shape;
    let (driver, timed) = if workloads::is_open(workload) {
        // Opening is the measurement: nothing is opened before the clock,
        // and the settings are read back only after it stops.
        let clock = workloads::Clock::start::<D>();
        let driver = open()?;
        let digest = workloads::first_read(&driver, rows)?;
        let timed = clock.done(1, digest);
        driver.verify()?;
        (driver, timed)
    } else {
        let driver = open()?;
        driver.verify()?;
        workloads::warm(&driver)?;
        let timed = workloads::run(&driver, workload, rows, pair, divisor)?;
        (driver, timed)
    };
    let latency: Option<Latency> = measure::latency(timed.latency_ns);
    let engine_version = driver.engine_version();
    Ok(Measured {
        schema: RECORD_SCHEMA.to_owned(),
        engine: engine_version
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned(),
        engine_version: engine_version.clone(),
        pair: pair.to_string(),
        rows,
        workload: workload.id.to_owned(),
        unit: workload.unit.to_owned(),
        work_divisor: divisor,
        ops: timed.ops,
        elapsed_ns: timed.elapsed.as_nanos() as u64,
        cpu_user_ns: timed.cpu_user_ns,
        cpu_sys_ns: timed.cpu_sys_ns,
        rss_peak_kib: measure::usage().max_rss_kib,
        io: serde_json::to_value(timed.io)?,
        latency_ns: latency.map(serde_json::to_value).transpose()?,
        counters: timed.counters,
        digest: timed.digest,
        settings: driver.settings()?,
    })
}
