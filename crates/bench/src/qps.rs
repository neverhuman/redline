//! Synthetic queries per second against Redline, SQLite, and Postgres.
//!
//! One schema, one seed, three engines. The load commits in batches of 500
//! rows. Point updates autocommit, so each one pays the engine's durable
//! commit. Reads reuse one prepared statement. Redline stays on Strict.
//! SQLite uses WAL and `synchronous=FULL`. Postgres uses `synchronous_commit=on`.

use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde::Serialize;

const CITIES: [&str; 16] = [
    "austin", "boston", "chicago", "denver", "eugene", "fresno", "geneva", "houston", "irvine",
    "juneau", "keene", "lansing", "miami", "newark", "oakland", "portland",
];
const BATCH: u64 = 500;
const CACHE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct Scale {
    pub accounts: u64,
    pub events: u64,
    pub iters: u64,
    pub updates: u64,
}

impl Scale {
    pub const SMOKE: Self = Self {
        accounts: 64,
        events: 256,
        iters: 40,
        updates: 8,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Engine {
    Redline,
    Sqlite,
    Postgres,
}

#[derive(Debug, Serialize)]
pub struct Phase {
    pub name: String,
    pub ops: u64,
    pub seconds: f64,
    pub per_sec: f64,
}

#[derive(Debug, Serialize)]
pub struct EngineReport {
    pub engine: String,
    pub version: String,
    pub durability: String,
    pub sum_amount: i64,
    pub phases: Vec<Phase>,
}

#[derive(Debug, Serialize)]
pub struct Ratio {
    pub phase: String,
    pub redline_per_sec: f64,
    pub sqlite_per_sec: f64,
    pub postgres_per_sec: f64,
    pub redline_over_sqlite: f64,
    pub redline_over_postgres: f64,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub scale: ScaleView,
    pub engines: Vec<EngineReport>,
    pub ratios: Vec<Ratio>,
}

#[derive(Debug, Serialize)]
pub struct ScaleView {
    pub accounts: u64,
    pub events: u64,
    pub iters: u64,
    pub updates: u64,
}

pub fn run(engines: &[Engine], scale: &Scale, postgres_url: Option<&str>) -> Result<Report> {
    let mut reports = Vec::with_capacity(engines.len());
    for engine in engines {
        let report = match engine {
            Engine::Redline => run_redline(scale)?,
            Engine::Sqlite => run_sqlite(scale)?,
            Engine::Postgres => {
                let url = postgres_url
                    .map(str::to_owned)
                    .or_else(|| std::env::var("REDLINE_TESTING_POSTGRES_URL").ok())
                    .context("postgres url missing; set REDLINE_TESTING_POSTGRES_URL")?;
                run_postgres(scale, &url)?
            }
        };
        let expect = expected_sum(scale.events);
        if report.sum_amount != expect {
            bail!(
                "{} sum {} != expected {expect}",
                report.engine,
                report.sum_amount
            );
        }
        reports.push(report);
    }
    let ratios = ratios_of(&reports);
    Ok(Report {
        scale: ScaleView {
            accounts: scale.accounts,
            events: scale.events,
            iters: scale.iters,
            updates: scale.updates,
        },
        engines: reports,
        ratios,
    })
}

fn ratios_of(reports: &[EngineReport]) -> Vec<Ratio> {
    let Some(redline) = reports.iter().find(|r| r.engine == "redline") else {
        return Vec::new();
    };
    let sqlite = reports.iter().find(|r| r.engine == "sqlite");
    let postgres = reports.iter().find(|r| r.engine == "postgres");
    redline
        .phases
        .iter()
        .map(|phase| {
            let sqlite_per_sec = sqlite
                .and_then(|r| r.phases.iter().find(|p| p.name == phase.name))
                .map(|p| p.per_sec)
                .unwrap_or(0.0);
            let postgres_per_sec = postgres
                .and_then(|r| r.phases.iter().find(|p| p.name == phase.name))
                .map(|p| p.per_sec)
                .unwrap_or(0.0);
            Ratio {
                phase: phase.name.clone(),
                redline_per_sec: phase.per_sec,
                sqlite_per_sec,
                postgres_per_sec,
                redline_over_sqlite: ratio(phase.per_sec, sqlite_per_sec),
                redline_over_postgres: ratio(phase.per_sec, postgres_per_sec),
            }
        })
        .collect()
}

fn ratio(left: f64, right: f64) -> f64 {
    if right == 0.0 { 0.0 } else { left / right }
}

pub fn expected_sum(events: u64) -> i64 {
    (0..events).map(amount_of).sum()
}

fn amount_of(id: u64) -> i64 {
    ((id.wrapping_mul(17)) % 10_000) as i64
}

fn account_of(id: u64, accounts: u64) -> i64 {
    (id % accounts.max(1)) as i64
}

fn ts_of(id: u64) -> i64 {
    1_700_000_000 + id as i64
}

fn mix(i: u64, n: u64) -> u64 {
    (i.wrapping_mul(11400714819323198485) >> 33) % n.max(1)
}

fn city_of(account: u64) -> &'static str {
    CITIES[(account as usize) % CITIES.len()]
}

fn time_iters(name: &str, ops: u64, mut body: impl FnMut(u64) -> Result<()>) -> Result<Phase> {
    let start = Instant::now();
    for i in 0..ops {
        body(i)?;
    }
    let seconds = start.elapsed().as_secs_f64();
    Ok(Phase {
        name: name.to_owned(),
        ops,
        seconds,
        per_sec: if seconds == 0.0 {
            0.0
        } else {
            ops as f64 / seconds
        },
    })
}

fn schema_sql() -> &'static str {
    "CREATE TABLE accounts(id INTEGER PRIMARY KEY, city TEXT NOT NULL, email TEXT NOT NULL);\n\
     CREATE TABLE events(id INTEGER PRIMARY KEY, account_id INTEGER NOT NULL, ts INTEGER NOT NULL, amount INTEGER NOT NULL);\n\
     CREATE INDEX events_account ON events(account_id);\n\
     CREATE INDEX accounts_city ON accounts(city);"
}

fn run_redline(scale: &Scale) -> Result<EngineReport> {
    let dir = tempfile::tempdir().context("redline temp dir")?;
    let path = dir.path().join("qps.redline");
    let mut options = redlinedb::OpenOptions::default();
    options.durability = redlinedb::Durability::Strict;
    options.memory.cache_bytes = CACHE_BYTES;
    let db = redlinedb::Database::open_with_options(&path, options)?;
    let mut conn = db.connect()?;
    conn.execute_batch(schema_sql())?;
    let mut phases = vec![load_redline(&mut conn, scale)?];
    let sum_amount = redline_query_i64(&mut conn, "SELECT sum(amount) FROM events", &[])?;
    phases.push(redline_point(&mut conn, scale)?);
    phases.push(redline_account(&mut conn, scale)?);
    phases.push(redline_recent(&mut conn, scale)?);
    phases.push(redline_city(&mut conn, scale)?);
    phases.push(redline_update(&mut conn, scale)?);
    Ok(EngineReport {
        engine: "redline".to_owned(),
        version: format!("redlinedb {}", env!("CARGO_PKG_VERSION")),
        durability: "strict".to_owned(),
        sum_amount,
        phases,
    })
}

fn load_redline(conn: &mut redlinedb::Connection, scale: &Scale) -> Result<Phase> {
    conn.begin(redlinedb::BeginMode::Immediate)?;
    {
        let mut stmt = conn.prepare("INSERT INTO accounts(id, city, email) VALUES (?, ?, ?)")?;
        for id in 0..scale.accounts {
            let email = format!("u{id}@example.test");
            stmt.reset()?;
            stmt.clear_bindings();
            stmt.bind_i64(1, id as i64)?;
            stmt.bind_text(2, city_of(id))?;
            stmt.bind_text(3, email.as_str())?;
            step_done(&mut stmt)?;
        }
    }
    conn.commit()?;
    let start = Instant::now();
    let mut done = 0u64;
    while done < scale.events {
        let end = (done + BATCH).min(scale.events);
        conn.begin(redlinedb::BeginMode::Immediate)?;
        {
            let mut stmt =
                conn.prepare("INSERT INTO events(id, account_id, ts, amount) VALUES (?, ?, ?, ?)")?;
            for id in done..end {
                stmt.reset()?;
                stmt.clear_bindings();
                stmt.bind_i64(1, id as i64)?;
                stmt.bind_i64(2, account_of(id, scale.accounts))?;
                stmt.bind_i64(3, ts_of(id))?;
                stmt.bind_i64(4, amount_of(id))?;
                step_done(&mut stmt)?;
            }
        }
        conn.commit()?;
        done = end;
    }
    let seconds = start.elapsed().as_secs_f64();
    Ok(Phase {
        name: "load_events".to_owned(),
        ops: scale.events,
        seconds,
        per_sec: scale.events as f64 / seconds.max(1e-9),
    })
}

fn redline_point(conn: &mut redlinedb::Connection, scale: &Scale) -> Result<Phase> {
    let mut stmt = conn.prepare("SELECT amount FROM events WHERE id = ?")?;
    time_iters("pk_point", scale.iters, |i| {
        let id = mix(i, scale.events) as i64;
        let got = step_i64(&mut stmt, &[id])?;
        std::hint::black_box(got);
        Ok(())
    })
}

fn redline_account(conn: &mut redlinedb::Connection, scale: &Scale) -> Result<Phase> {
    let mut stmt = conn.prepare("SELECT count(*) FROM events WHERE account_id = ?")?;
    time_iters("index_count", scale.iters, |i| {
        let id = mix(i, scale.accounts) as i64;
        let got = step_i64(&mut stmt, &[id])?;
        std::hint::black_box(got);
        Ok(())
    })
}

fn redline_recent(conn: &mut redlinedb::Connection, scale: &Scale) -> Result<Phase> {
    let mut stmt =
        conn.prepare("SELECT id FROM events WHERE account_id = ? ORDER BY ts DESC LIMIT 16")?;
    time_iters("index_recent", scale.iters, |i| {
        let id = mix(i, scale.accounts) as i64;
        let n = step_count(&mut stmt, &[id])?;
        std::hint::black_box(n);
        Ok(())
    })
}

fn redline_city(conn: &mut redlinedb::Connection, scale: &Scale) -> Result<Phase> {
    let mut stmt = conn.prepare(
        "SELECT coalesce(sum(e.amount), 0) FROM accounts a JOIN events e ON e.account_id = a.id WHERE a.city = ?",
    )?;
    time_iters("join_city_sum", scale.iters, |i| {
        let city = city_of(mix(i, CITIES.len() as u64));
        stmt.reset()?;
        stmt.clear_bindings();
        stmt.bind_text(1, city)?;
        let got = match stmt.step()? {
            redlinedb::Step::Row(row) => row.get::<i64>(0)?,
            redlinedb::Step::Done => bail!("join returned no row"),
        };
        std::hint::black_box(got);
        Ok(())
    })
}

fn redline_update(conn: &mut redlinedb::Connection, scale: &Scale) -> Result<Phase> {
    let mut stmt = conn.prepare("UPDATE events SET amount = amount + 1 WHERE id = ?")?;
    time_iters("autocommit_update", scale.updates, |i| {
        let id = mix(i, scale.events) as i64;
        stmt.reset()?;
        stmt.clear_bindings();
        stmt.bind_i64(1, id)?;
        step_done(&mut stmt)?;
        Ok(())
    })
}

fn step_done(stmt: &mut redlinedb::Statement<'_>) -> Result<()> {
    match stmt.step()? {
        redlinedb::Step::Row(_) | redlinedb::Step::Done => Ok(()),
    }
}

fn step_i64(stmt: &mut redlinedb::Statement<'_>, params: &[i64]) -> Result<i64> {
    stmt.reset()?;
    stmt.clear_bindings();
    for (index, value) in params.iter().enumerate() {
        stmt.bind_i64(index + 1, *value)?;
    }
    match stmt.step()? {
        redlinedb::Step::Row(row) => Ok(row.get(0)?),
        redlinedb::Step::Done => bail!("query returned no row"),
    }
}

fn step_count(stmt: &mut redlinedb::Statement<'_>, params: &[i64]) -> Result<usize> {
    stmt.reset()?;
    stmt.clear_bindings();
    for (index, value) in params.iter().enumerate() {
        stmt.bind_i64(index + 1, *value)?;
    }
    let mut n = 0usize;
    loop {
        match stmt.step()? {
            redlinedb::Step::Row(_) => n += 1,
            redlinedb::Step::Done => return Ok(n),
        }
    }
}

fn redline_query_i64(conn: &mut redlinedb::Connection, sql: &str, params: &[i64]) -> Result<i64> {
    let mut stmt = conn.prepare(sql)?;
    step_i64(&mut stmt, params)
}

fn run_sqlite(scale: &Scale) -> Result<EngineReport> {
    let dir = tempfile::tempdir().context("sqlite temp dir")?;
    let path = dir.path().join("qps.sqlite");
    let conn = open_sqlite(&path)?;
    conn.execute_batch(schema_sql())?;
    let load = load_sqlite(&conn, scale)?;
    let sum_amount: i64 = conn.query_row("SELECT sum(amount) FROM events", [], |row| row.get(0))?;
    let mut phases = vec![load];
    phases.push(sqlite_point(&conn, scale)?);
    phases.push(sqlite_account(&conn, scale)?);
    phases.push(sqlite_recent(&conn, scale)?);
    phases.push(sqlite_city(&conn, scale)?);
    phases.push(sqlite_update(&conn, scale)?);
    let version: String = conn.query_row("SELECT sqlite_version()", [], |row| row.get(0))?;
    Ok(EngineReport {
        engine: "sqlite".to_owned(),
        version,
        durability: "wal+synchronous=full".to_owned(),
        sum_amount,
        phases,
    })
}

fn open_sqlite(path: &Path) -> Result<rusqlite::Connection> {
    let conn = rusqlite::Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.pragma_update(None, "cache_size", &(-(CACHE_BYTES as i64) / 1024))?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    Ok(conn)
}

fn load_sqlite(conn: &rusqlite::Connection, scale: &Scale) -> Result<Phase> {
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare("INSERT INTO accounts(id, city, email) VALUES (?1, ?2, ?3)")?;
        for id in 0..scale.accounts {
            let email = format!("u{id}@example.test");
            stmt.execute(rusqlite::params![id as i64, city_of(id), email])?;
        }
    }
    tx.commit()?;
    let start = Instant::now();
    let mut done = 0u64;
    while done < scale.events {
        let end = (done + BATCH).min(scale.events);
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO events(id, account_id, ts, amount) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for id in done..end {
                stmt.execute(rusqlite::params![
                    id as i64,
                    account_of(id, scale.accounts),
                    ts_of(id),
                    amount_of(id)
                ])?;
            }
        }
        tx.commit()?;
        done = end;
    }
    let seconds = start.elapsed().as_secs_f64();
    Ok(Phase {
        name: "load_events".to_owned(),
        ops: scale.events,
        seconds,
        per_sec: scale.events as f64 / seconds.max(1e-9),
    })
}

fn sqlite_point(conn: &rusqlite::Connection, scale: &Scale) -> Result<Phase> {
    let mut stmt = conn.prepare("SELECT amount FROM events WHERE id = ?1")?;
    time_iters("pk_point", scale.iters, |i| {
        let id = mix(i, scale.events) as i64;
        let got: i64 = stmt.query_row(rusqlite::params![id], |row| row.get(0))?;
        std::hint::black_box(got);
        Ok(())
    })
}

fn sqlite_account(conn: &rusqlite::Connection, scale: &Scale) -> Result<Phase> {
    let mut stmt = conn.prepare("SELECT count(*) FROM events WHERE account_id = ?1")?;
    time_iters("index_count", scale.iters, |i| {
        let id = mix(i, scale.accounts) as i64;
        let got: i64 = stmt.query_row(rusqlite::params![id], |row| row.get(0))?;
        std::hint::black_box(got);
        Ok(())
    })
}

fn sqlite_recent(conn: &rusqlite::Connection, scale: &Scale) -> Result<Phase> {
    let mut stmt =
        conn.prepare("SELECT id FROM events WHERE account_id = ?1 ORDER BY ts DESC LIMIT 16")?;
    time_iters("index_recent", scale.iters, |i| {
        let id = mix(i, scale.accounts) as i64;
        let mut rows = stmt.query(rusqlite::params![id])?;
        let mut n = 0usize;
        while rows.next()?.is_some() {
            n += 1;
        }
        std::hint::black_box(n);
        Ok(())
    })
}

fn sqlite_city(conn: &rusqlite::Connection, scale: &Scale) -> Result<Phase> {
    let mut stmt = conn.prepare(
        "SELECT coalesce(sum(e.amount), 0) FROM accounts a JOIN events e ON e.account_id = a.id WHERE a.city = ?1",
    )?;
    time_iters("join_city_sum", scale.iters, |i| {
        let city = city_of(mix(i, CITIES.len() as u64));
        let got: i64 = stmt.query_row(rusqlite::params![city], |row| row.get(0))?;
        std::hint::black_box(got);
        Ok(())
    })
}

fn sqlite_update(conn: &rusqlite::Connection, scale: &Scale) -> Result<Phase> {
    let mut stmt = conn.prepare("UPDATE events SET amount = amount + 1 WHERE id = ?1")?;
    time_iters("autocommit_update", scale.updates, |i| {
        let id = mix(i, scale.events) as i64;
        stmt.execute(rusqlite::params![id])?;
        Ok(())
    })
}

fn run_postgres(scale: &Scale, url: &str) -> Result<EngineReport> {
    let mut client = postgres::Client::connect(url, postgres::NoTls).context("connect postgres")?;
    client.batch_execute(
        "DROP SCHEMA IF EXISTS qps_bench CASCADE; CREATE SCHEMA qps_bench; SET search_path TO qps_bench; SET synchronous_commit = on;",
    )?;
    let result = run_postgres_on(scale, &mut client);
    let _ = client.batch_execute("DROP SCHEMA IF EXISTS qps_bench CASCADE");
    result
}

fn run_postgres_on(scale: &Scale, client: &mut postgres::Client) -> Result<EngineReport> {
    client.batch_execute(schema_sql())?;
    let load = load_postgres(client, scale)?;
    let sum_amount: i64 = client
        .query_one("SELECT coalesce(sum(amount), 0) FROM events", &[])?
        .get(0);
    let mut phases = vec![load];
    phases.push(postgres_point(client, scale)?);
    phases.push(postgres_account(client, scale)?);
    phases.push(postgres_recent(client, scale)?);
    phases.push(postgres_city(client, scale)?);
    phases.push(postgres_update(client, scale)?);
    let version: String = client.query_one("SHOW server_version", &[])?.get(0);
    Ok(EngineReport {
        engine: "postgres".to_owned(),
        version,
        durability: "synchronous_commit=on".to_owned(),
        sum_amount,
        phases,
    })
}

fn load_postgres(client: &mut postgres::Client, scale: &Scale) -> Result<Phase> {
    let mut tx = client.transaction()?;
    for id in 0..scale.accounts {
        let email = format!("u{id}@example.test");
        tx.execute(
            "INSERT INTO accounts(id, city, email) VALUES ($1, $2, $3)",
            &[&(id as i32), &city_of(id), &email],
        )?;
    }
    tx.commit()?;
    let start = Instant::now();
    let mut done = 0u64;
    while done < scale.events {
        let end = (done + BATCH).min(scale.events);
        let mut tx = client.transaction()?;
        for id in done..end {
            tx.execute(
                "INSERT INTO events(id, account_id, ts, amount) VALUES ($1, $2, $3, $4)",
                &[
                    &(id as i32),
                    &(account_of(id, scale.accounts) as i32),
                    &(ts_of(id) as i32),
                    &(amount_of(id) as i32),
                ],
            )?;
        }
        tx.commit()?;
        done = end;
    }
    let seconds = start.elapsed().as_secs_f64();
    Ok(Phase {
        name: "load_events".to_owned(),
        ops: scale.events,
        seconds,
        per_sec: scale.events as f64 / seconds.max(1e-9),
    })
}

fn postgres_point(client: &mut postgres::Client, scale: &Scale) -> Result<Phase> {
    let stmt = client.prepare("SELECT amount FROM events WHERE id = $1")?;
    time_iters("pk_point", scale.iters, |i| {
        let id = mix(i, scale.events) as i32;
        let got: i32 = client.query_one(&stmt, &[&id])?.get(0);
        std::hint::black_box(got);
        Ok(())
    })
}

fn postgres_account(client: &mut postgres::Client, scale: &Scale) -> Result<Phase> {
    let stmt = client.prepare("SELECT count(*) FROM events WHERE account_id = $1")?;
    time_iters("index_count", scale.iters, |i| {
        let id = mix(i, scale.accounts) as i32;
        let got: i64 = client.query_one(&stmt, &[&id])?.get(0);
        std::hint::black_box(got);
        Ok(())
    })
}

fn postgres_recent(client: &mut postgres::Client, scale: &Scale) -> Result<Phase> {
    let stmt =
        client.prepare("SELECT id FROM events WHERE account_id = $1 ORDER BY ts DESC LIMIT 16")?;
    time_iters("index_recent", scale.iters, |i| {
        let id = mix(i, scale.accounts) as i32;
        let n = client.query(&stmt, &[&id])?.len();
        std::hint::black_box(n);
        Ok(())
    })
}

fn postgres_city(client: &mut postgres::Client, scale: &Scale) -> Result<Phase> {
    let stmt = client.prepare(
        "SELECT coalesce(sum(e.amount), 0) FROM accounts a JOIN events e ON e.account_id = a.id WHERE a.city = $1",
    )?;
    time_iters("join_city_sum", scale.iters, |i| {
        let city = city_of(mix(i, CITIES.len() as u64));
        let got: i64 = client.query_one(&stmt, &[&city])?.get(0);
        std::hint::black_box(got);
        Ok(())
    })
}

fn postgres_update(client: &mut postgres::Client, scale: &Scale) -> Result<Phase> {
    let stmt = client.prepare("UPDATE events SET amount = amount + 1 WHERE id = $1")?;
    time_iters("autocommit_update", scale.updates, |i| {
        let id = mix(i, scale.events) as i32;
        client.execute(&stmt, &[&id])?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_redline_and_sqlite_match_and_move() {
        let scale = Scale::SMOKE;
        let report = run(&[Engine::Redline, Engine::Sqlite], &scale, None).expect("qps smoke");
        assert_eq!(report.engines.len(), 2);
        for engine in &report.engines {
            assert_eq!(
                engine.sum_amount,
                expected_sum(scale.events),
                "{}",
                engine.engine
            );
            assert!(
                engine
                    .phases
                    .iter()
                    .any(|p| p.name == "pk_point" && p.per_sec > 0.0),
                "{}",
                engine.engine
            );
            assert!(
                engine
                    .phases
                    .iter()
                    .any(|p| p.name == "load_events" && p.ops == scale.events),
                "{}",
                engine.engine
            );
        }
        assert!(
            report
                .ratios
                .iter()
                .any(|r| r.phase == "pk_point" && r.redline_over_sqlite > 0.0)
        );
    }

    #[test]
    fn city_join_sum_matches_the_seed() {
        let accounts = 32u64;
        let events = 200u64;
        let dir = tempfile::tempdir().unwrap();
        let mut options = redlinedb::OpenOptions::default();
        options.durability = redlinedb::Durability::UnsafeDev;
        let db = redlinedb::Database::open_with_options(dir.path().join("join.redline"), options)
            .unwrap();
        let mut conn = db.connect().unwrap();
        conn.execute_batch(schema_sql()).unwrap();
        load_redline(
            &mut conn,
            &Scale {
                accounts,
                events,
                iters: 1,
                updates: 0,
            },
        )
        .unwrap();
        let forward = redline_query_i64(
            &mut conn,
            "SELECT coalesce(sum(e.amount), 0) FROM accounts a JOIN events e ON e.account_id = a.id WHERE a.city = 'austin'",
            &[],
        )
        .unwrap();
        let reverse = redline_query_i64(
            &mut conn,
            "SELECT coalesce(sum(e.amount), 0) FROM events e JOIN accounts a ON a.id = e.account_id WHERE a.city = 'austin'",
            &[],
        )
        .unwrap();
        let expect: i64 = (0..events)
            .filter(|id| city_of(account_of(*id, accounts) as u64) == "austin")
            .map(amount_of)
            .sum();
        assert_eq!(forward, expect);
        assert_eq!(reverse, expect);
    }
}
