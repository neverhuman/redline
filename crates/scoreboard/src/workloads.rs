//! The workloads. Each does a fixed amount of work, identical for every
//! engine and version, never "as much as fits in a time budget", so a
//! faster engine cannot change what a later measurement sees.

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use serde::Serialize;

use crate::driver::{Arg, Driver, Rows, one};
use crate::image::{ImageKind, NOTE, Scale, VALUE, account_of, amount_of, mix};
use crate::pair::Pair;

/// One entry of the catalog.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Workload {
    pub id: &'static str,
    /// What `ops` counts; throughput is ops per second.
    pub unit: &'static str,
    pub image: ImageKind,
    /// Whether the workload writes; only writers run in the strict pair.
    pub writes: bool,
    pub summary: &'static str,
}

const fn w(id: &'static str, unit: &'static str, writes: bool, summary: &'static str) -> Workload {
    Workload {
        id,
        unit,
        image: ImageKind::Base,
        writes,
        summary,
    }
}

pub const CATALOG: &[Workload] = &[
    w(
        "insert_autocommit",
        "txn",
        true,
        "single-row INSERT, one transaction each",
    ),
    w(
        "insert_batch100",
        "row",
        true,
        "INSERT, 100 rows per transaction",
    ),
    w(
        "insert_bulk_1txn",
        "row",
        true,
        "INSERT of N rows into an empty indexed table, one transaction",
    ),
    w(
        "point_pk_prepared",
        "stmt",
        false,
        "SELECT by INTEGER PRIMARY KEY, prepared",
    ),
    w(
        "point_pk_sql_text",
        "stmt",
        false,
        "SELECT by INTEGER PRIMARY KEY, a new SQL string each time",
    ),
    w(
        "range_pk_sum_100",
        "row",
        false,
        "sum over a 100-row rowid range",
    ),
    w(
        "range_pk_rows_100",
        "row",
        false,
        "100-row rowid range, three columns read",
    ),
    w(
        "index_count",
        "stmt",
        false,
        "count(*) by a secondary index",
    ),
    w(
        "index_rows",
        "stmt",
        false,
        "rows by a secondary index, two columns read",
    ),
    w(
        "full_scan_sum",
        "row",
        false,
        "sum over a filtered full scan",
    ),
    w("group_by", "row", false, "GROUP BY over the whole table"),
    w(
        "top10",
        "row",
        false,
        "ORDER BY ... LIMIT 10 over the whole table",
    ),
    w(
        "join_point",
        "stmt",
        false,
        "join of one account to its events",
    ),
    w(
        "join_group_by_city",
        "row",
        false,
        "join of every event to its account, grouped",
    ),
    w(
        "update_pk_autocommit",
        "txn",
        true,
        "UPDATE by INTEGER PRIMARY KEY, one transaction each",
    ),
    w(
        "update_pk_1txn",
        "row",
        true,
        "UPDATE by INTEGER PRIMARY KEY, one transaction",
    ),
    w(
        "update_indexed_1txn",
        "row",
        true,
        "UPDATE of an indexed column, one transaction",
    ),
    w(
        "delete_pk_1txn",
        "row",
        true,
        "DELETE by INTEGER PRIMARY KEY, one transaction",
    ),
    w(
        "open_first_read",
        "open",
        false,
        "open a checkpointed database and read one row",
    ),
    Workload {
        id: "open_after_updates",
        unit: "open",
        image: ImageKind::AfterUpdates,
        writes: false,
        summary: "open after 5,000 updates left in the log and read one row",
    },
];

pub fn find(id: &str) -> Result<Workload> {
    match CATALOG.iter().find(|workload| workload.id == id) {
        Some(workload) => Ok(*workload),
        None => bail!("unknown workload {id}"),
    }
}

/// Whether the workload measures opening the database, so the case must
/// not open it before timing.
pub fn is_open(workload: &Workload) -> bool {
    workload.unit == "open"
}

/// Autocommit writes per case. Strict syncs every commit, so it does fewer.
fn autocommit_ops(pair: Pair) -> u64 {
    match pair {
        Pair::Normal => 2_000,
        Pair::Strict => 500,
    }
}

/// `ops` divided by the run's work divisor, at least 1. Published runs use
/// divisor 1, the catalog's full fixed work; tests use a larger one.
fn scaled(ops: u64, divisor: u64) -> u64 {
    (ops / divisor.max(1)).max(1)
}

/// The timed part of one repetition.
#[derive(Debug, Default)]
pub struct Timed {
    pub ops: u64,
    pub elapsed: Duration,
    /// A value both engines must agree on: the rows read, or the state the
    /// writes left.
    pub digest: i64,
    /// Per-operation latency in nanoseconds, where the workload records it.
    pub latency_ns: Vec<u64>,
}

/// Latency is sampled every `LATENCY_EVERY` fast operations, so reading
/// the clock stays a small share of a sub-microsecond statement.
const LATENCY_EVERY: u64 = 16;

struct Clock {
    start: Instant,
    latency_ns: Vec<u64>,
}

impl Clock {
    fn start() -> Self {
        Self {
            start: Instant::now(),
            latency_ns: Vec::new(),
        }
    }

    fn done(self, ops: u64, digest: i64) -> Timed {
        Timed {
            ops,
            elapsed: self.start.elapsed(),
            digest,
            latency_ns: self.latency_ns,
        }
    }
}

/// Time `op` for every step when `every` is 1, else for one step in
/// `every`.
fn step<T>(clock: &mut Clock, i: u64, every: u64, op: impl FnOnce() -> Result<T>) -> Result<T> {
    if i.is_multiple_of(every) {
        let begin = Instant::now();
        let out = op()?;
        clock.latency_ns.push(begin.elapsed().as_nanos() as u64);
        Ok(out)
    } else {
        op()
    }
}

/// Run `workload` on an open database of `rows` events.
pub fn run<D: Driver>(
    driver: &D,
    workload: &Workload,
    rows: u64,
    pair: Pair,
    divisor: u64,
) -> Result<Timed> {
    let scale = Scale::new(rows);
    let n = scale.events;
    match workload.id {
        "insert_autocommit" => {
            let ops = scaled(autocommit_ops(pair), divisor);
            let mut stmt = driver.prepare("INSERT INTO kv(k, v) VALUES (?, ?)")?;
            let mut clock = Clock::start();
            for i in 0..ops {
                step(&mut clock, i, 1, || {
                    D::run(
                        &mut stmt,
                        &[Arg::Int(10_000_000 + i as i64), Arg::Text(VALUE)],
                        0,
                    )
                })?;
            }
            let timed = clock.done(ops, 0);
            drop(stmt);
            finish(driver, timed, Left::Kv)
        }
        "insert_batch100" => {
            let ops = scaled((n / 5) as u64, divisor);
            let mut stmt = driver.prepare("INSERT INTO kv(k, v) VALUES (?, ?)")?;
            let clock = Clock::start();
            let mut i = 0;
            while i < ops {
                driver.begin()?;
                for k in i..(i + 100).min(ops) {
                    D::run(
                        &mut stmt,
                        &[Arg::Int(20_000_000 + k as i64), Arg::Text(VALUE)],
                        0,
                    )?;
                }
                driver.commit()?;
                i += 100;
            }
            let timed = clock.done(ops, 0);
            drop(stmt);
            finish(driver, timed, Left::Kv)
        }
        "insert_bulk_1txn" => {
            let ops = n as u64;
            let mut stmt = driver.prepare(
                "INSERT INTO bulk(id, account_id, ts, amount, note) VALUES (?, ?, ?, ?, ?)",
            )?;
            let clock = Clock::start();
            driver.begin()?;
            for id in 1..=n {
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
            driver.commit()?;
            let timed = clock.done(ops, 0);
            drop(stmt);
            finish(driver, timed, Left::Bulk)
        }
        "point_pk_prepared" => {
            let ops = scaled(200_000, divisor);
            let mut stmt = driver.prepare("SELECT amount FROM events WHERE id = ?")?;
            let mut clock = Clock::start();
            let mut rows = Rows::default();
            for i in 0..ops {
                let got = step(&mut clock, i, LATENCY_EVERY, || {
                    D::run(&mut stmt, &[Arg::Int(1 + mix(i, n))], 1)
                })?;
                rows.add(got);
            }
            Ok(clock.done(ops, rows.digest))
        }
        "point_pk_sql_text" => {
            let ops = scaled(20_000, divisor);
            let mut clock = Clock::start();
            let mut rows = Rows::default();
            for i in 0..ops {
                // The id is this program's own integer, not input; each string
                // is new so neither engine's statement cache can answer it.
                let sql = format!("SELECT amount FROM events WHERE id = {}", 1 + mix(i, n)); // jankurai:allow HLT-023-INPUT-BOUNDARY-GAP reason=benchmark-generated-integer-literal-defeats-statement-cache-by-design expires=2027-09-30
                let got = step(&mut clock, i, LATENCY_EVERY, || {
                    let mut stmt = driver.prepare(&sql)?;
                    D::run(&mut stmt, &[], 1)
                })?;
                rows.add(got);
            }
            Ok(clock.done(ops, rows.digest))
        }
        "range_pk_sum_100" | "range_pk_rows_100" => {
            let (queries, sql, columns) = if workload.id == "range_pk_sum_100" {
                (
                    1_000u64,
                    "SELECT sum(amount) FROM events WHERE id BETWEEN ? AND ?",
                    1,
                )
            } else {
                (
                    500u64,
                    "SELECT id, amount, note FROM events WHERE id >= ? AND id < ?",
                    3,
                )
            };
            let queries = scaled(queries, divisor);
            let span = 100.min(n);
            let mut stmt = driver.prepare(sql)?;
            let mut clock = Clock::start();
            let mut rows = Rows::default();
            for i in 0..queries {
                let low = 1 + mix(i, n - span + 1);
                let high = if columns == 1 {
                    low + span - 1
                } else {
                    low + span
                };
                let got = step(&mut clock, i, 1, || {
                    D::run(&mut stmt, &[Arg::Int(low), Arg::Int(high)], columns)
                })?;
                rows.add(got);
            }
            Ok(clock.done(queries * span as u64, rows.digest))
        }
        "index_count" | "index_rows" => {
            let (ops, sql, columns) = if workload.id == "index_count" {
                (
                    20_000u64,
                    "SELECT count(*) FROM events WHERE account_id = ?",
                    1,
                )
            } else {
                (
                    5_000u64,
                    "SELECT id, amount FROM events WHERE account_id = ?",
                    2,
                )
            };
            let ops = scaled(ops, divisor);
            let mut stmt = driver.prepare(sql)?;
            let mut clock = Clock::start();
            let mut rows = Rows::default();
            for i in 0..ops {
                let got = step(&mut clock, i, LATENCY_EVERY, || {
                    D::run(&mut stmt, &[Arg::Int(1 + mix(i, scale.accounts))], columns)
                })?;
                rows.add(got);
            }
            Ok(clock.done(ops, rows.digest))
        }
        "full_scan_sum" | "group_by" | "top10" | "join_group_by_city" => {
            let (passes, sql, columns): (u64, &str, usize) = match workload.id {
                "full_scan_sum" => (5, "SELECT sum(amount) FROM events WHERE ts > 0", 1),
                "group_by" => (
                    3,
                    "SELECT account_id, sum(amount) FROM events GROUP BY account_id",
                    2,
                ),
                "top10" => (
                    5,
                    "SELECT id FROM events ORDER BY amount DESC, id DESC LIMIT 10",
                    1,
                ),
                _ => (
                    1,
                    "SELECT a.city, sum(e.amount) FROM accounts a JOIN events e ON e.account_id = a.id GROUP BY a.city",
                    2,
                ),
            };
            let mut stmt = driver.prepare(sql)?;
            let mut clock = Clock::start();
            let mut rows = Rows::default();
            for i in 0..passes {
                let got = step(&mut clock, i, 1, || D::run(&mut stmt, &[], columns))?;
                rows.add(got);
            }
            Ok(clock.done(passes * n as u64, rows.digest))
        }
        "join_point" => {
            let ops = scaled(2_000, divisor);
            let mut stmt = driver.prepare(
                "SELECT coalesce(sum(e.amount), 0) FROM accounts a JOIN events e ON e.account_id = a.id WHERE a.id = ?",
            )?;
            let mut clock = Clock::start();
            let mut rows = Rows::default();
            for i in 0..ops {
                let got = step(&mut clock, i, 1, || {
                    D::run(&mut stmt, &[Arg::Int(1 + mix(i, scale.accounts))], 1)
                })?;
                rows.add(got);
            }
            Ok(clock.done(ops, rows.digest))
        }
        "update_pk_autocommit" | "update_pk_1txn" => {
            let autocommit = workload.id == "update_pk_autocommit";
            let ops = scaled(
                if autocommit {
                    autocommit_ops(pair)
                } else {
                    2_000
                },
                divisor,
            );
            let mut stmt = driver.prepare("UPDATE events SET amount = amount + 1 WHERE id = ?")?;
            let mut clock = Clock::start();
            if !autocommit {
                driver.begin()?;
            }
            for i in 0..ops {
                let every = if autocommit { 1 } else { LATENCY_EVERY };
                step(&mut clock, i, every, || {
                    D::run(&mut stmt, &[Arg::Int(1 + mix(i, n))], 0)
                })?;
            }
            if !autocommit {
                driver.commit()?;
            }
            let timed = clock.done(ops, 0);
            drop(stmt);
            finish(driver, timed, Left::EventAmounts)
        }
        "update_indexed_1txn" => {
            let ops = scaled(2_000, divisor);
            let mut stmt = driver.prepare("UPDATE events SET account_id = ? WHERE id = ?")?;
            let clock = Clock::start();
            driver.begin()?;
            for i in 0..ops {
                D::run(
                    &mut stmt,
                    &[
                        Arg::Int(1 + mix(i + 7, scale.accounts)),
                        Arg::Int(1 + mix(i, n)),
                    ],
                    0,
                )?;
            }
            driver.commit()?;
            let timed = clock.done(ops, 0);
            drop(stmt);
            finish(driver, timed, Left::EventAccounts)
        }
        "delete_pk_1txn" => {
            let ops = scaled(2_000, divisor).min(scale.kv as u64);
            let mut stmt = driver.prepare("DELETE FROM kv WHERE k = ?")?;
            let clock = Clock::start();
            driver.begin()?;
            for k in 1..=ops {
                D::run(&mut stmt, &[Arg::Int(k as i64)], 0)?;
            }
            driver.commit()?;
            let timed = clock.done(ops, 0);
            drop(stmt);
            finish(driver, timed, Left::Kv)
        }
        other => bail!("workload {other} has no body"),
    }
}

/// The state a write workload leaves, which both engines must agree on.
#[derive(Clone, Copy)]
enum Left {
    Kv,
    Bulk,
    EventAmounts,
    EventAccounts,
}

impl Left {
    /// The query and how many columns of its one row the digest sums.
    fn query(self) -> (&'static str, usize) {
        match self {
            Self::Kv => ("SELECT count(*), sum(k) FROM kv", 2),
            Self::Bulk => ("SELECT count(*), sum(amount) FROM bulk", 2),
            Self::EventAmounts => ("SELECT sum(amount) FROM events", 1),
            Self::EventAccounts => ("SELECT sum(account_id) FROM events", 1),
        }
    }
}

/// Add the digest of the state a write workload left.
fn finish<D: Driver>(driver: &D, mut timed: Timed, left: Left) -> Result<Timed> {
    let (sql, columns) = left.query();
    let mut stmt = driver.prepare(sql)?;
    timed.digest = D::run(&mut stmt, &[], columns)?.digest;
    Ok(timed)
}

/// Untimed reads before a repetition, so both engines start with the
/// tables in their page cache.
pub fn warm<D: Driver>(driver: &D) -> Result<()> {
    for sql in [
        "SELECT count(*), sum(amount) FROM events",
        "SELECT count(*) FROM accounts",
        "SELECT count(*) FROM kv",
    ] {
        one(driver, sql, &[])?;
    }
    Ok(())
}

/// The first read of an open workload.
pub fn first_read<D: Driver>(driver: &D, rows: u64) -> Result<i64> {
    let scale = Scale::new(rows);
    one(
        driver,
        "SELECT amount FROM events WHERE id = ?",
        &[Arg::Int(1 + scale.events / 2)],
    )
}
