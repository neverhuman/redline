//! Test databases ("images"). Each is built once, by one seeded generator,
//! and every repetition works on a fresh copy, so no workload inherits
//! another's rows, log or cache.

use std::fs;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::driver::{Arg, Driver};

pub const SCHEMA: &str = "\
CREATE TABLE accounts(id INTEGER PRIMARY KEY, city TEXT NOT NULL, email TEXT NOT NULL);
CREATE TABLE events(id INTEGER PRIMARY KEY, account_id INTEGER NOT NULL, ts INTEGER NOT NULL, amount INTEGER NOT NULL, note TEXT);
CREATE INDEX events_account ON events(account_id);
CREATE INDEX accounts_city ON accounts(city);
CREATE TABLE kv(k INTEGER PRIMARY KEY, v TEXT);
CREATE TABLE bulk(id INTEGER PRIMARY KEY, account_id INTEGER NOT NULL, ts INTEGER NOT NULL, amount INTEGER NOT NULL, note TEXT);
CREATE INDEX bulk_account ON bulk(account_id);";

pub const CITIES: [&str; 8] = [
    "austin", "boise", "chicago", "denver", "el paso", "fresno", "gary", "houston",
];
pub const NOTE: &str = "note-abcdefghijklmnop";
pub const VALUE: &str = "value-0123456789";

/// Updates applied to the `after-updates` image, left in the log.
pub const IMAGE_UPDATES: u64 = 5_000;

/// Which image a workload starts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum ImageKind {
    /// Loaded, analyzed and checkpointed.
    Base,
    /// The base image plus [`IMAGE_UPDATES`] autocommit updates, closed
    /// without a checkpoint, for the reopen workload.
    AfterUpdates,
}

/// Rows in each table for a scale of `rows` events.
#[derive(Clone, Copy, Debug)]
pub struct Scale {
    pub events: i64,
    pub accounts: i64,
    pub kv: i64,
}

impl Scale {
    pub fn new(rows: u64) -> Self {
        let events = rows.max(10) as i64;
        Self {
            events,
            accounts: (events / 10).max(1),
            kv: (events / 5).max(1),
        }
    }
}

/// A well-mixed index in `0..n` for step `i`: Fibonacci hashing.
pub fn mix(i: u64, n: i64) -> i64 {
    ((i.wrapping_mul(11_400_714_819_323_198_485) >> 33) % (n.max(1) as u64)) as i64
}

pub fn account_of(event: i64, scale: Scale) -> i64 {
    1 + event % scale.accounts
}

pub fn amount_of(event: i64) -> i64 {
    (event * 17) % 10_000
}

/// Create the schema and load every table, each in one transaction, then
/// analyze and checkpoint.
pub fn build<D: Driver>(driver: &D, rows: u64) -> Result<()> {
    let scale = Scale::new(rows);
    driver.batch(SCHEMA)?;
    driver.begin()?;
    {
        let mut stmt = driver.prepare("INSERT INTO accounts(id, city, email) VALUES (?, ?, ?)")?;
        for id in 1..=scale.accounts {
            let email = format!("u{id}@example.test");
            D::run(
                &mut stmt,
                &[
                    Arg::Int(id),
                    Arg::Text(CITIES[(id % 8) as usize]),
                    Arg::Text(&email),
                ],
                0,
            )?;
        }
    }
    driver.commit()?;
    driver.begin()?;
    {
        let mut stmt = driver.prepare(
            "INSERT INTO events(id, account_id, ts, amount, note) VALUES (?, ?, ?, ?, ?)",
        )?;
        for id in 1..=scale.events {
            D::run(
                &mut stmt,
                &[
                    Arg::Int(id),
                    Arg::Int(account_of(id, scale)),
                    Arg::Int(1_700_000_000 + id),
                    Arg::Int(amount_of(id)),
                    Arg::Text(NOTE),
                ],
                0,
            )?;
        }
    }
    driver.commit()?;
    driver.begin()?;
    {
        let mut stmt = driver.prepare("INSERT INTO kv(k, v) VALUES (?, ?)")?;
        for k in 1..=scale.kv {
            D::run(&mut stmt, &[Arg::Int(k), Arg::Text(VALUE)], 0)?;
        }
    }
    driver.commit()?;
    driver.batch("ANALYZE")?;
    driver.checkpoint()
}

/// Apply [`IMAGE_UPDATES`] autocommit updates, leaving them in the log.
pub fn apply_updates<D: Driver>(driver: &D, rows: u64) -> Result<()> {
    let scale = Scale::new(rows);
    let mut stmt = driver.prepare("UPDATE events SET amount = amount + 1 WHERE id = ?")?;
    for i in 0..IMAGE_UPDATES {
        D::run(&mut stmt, &[Arg::Int(1 + mix(i, scale.events))], 0)?;
    }
    Ok(())
}

/// Copy the directory `from` to `to`, which must not exist.
pub fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).with_context(|| format!("create {}", to.display()))?;
    for entry in fs::read_dir(from).with_context(|| format!("read {}", from.display()))? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)
                .with_context(|| format!("copy {}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// sha256 over every file under `dir`, by relative path in sorted order.
pub fn tree_sha256(dir: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect_files(dir, dir, &mut files)?;
    files.sort();
    let mut hasher = Sha256::new();
    for relative in files {
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        let mut file = fs::File::open(dir.join(&relative))?;
        let mut buf = [0u8; 1 << 16];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            collect_files(root, &entry.path(), out)?;
        } else {
            let relative = entry
                .path()
                .strip_prefix(root)?
                .to_string_lossy()
                .into_owned();
            out.push(relative);
        }
    }
    Ok(())
}
