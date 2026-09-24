//! Synthetic queries-per-second comparison.
//!
//! `qps_compare` loads 2,000 accounts and 20,000 events, then times reads and
//! autocommit updates. Raise `--events` for a larger set.

use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;

use redlinedb_bench::qps::{Engine, Scale, run};

#[derive(Parser)]
struct Args {
    #[arg(long, value_delimiter = ',', default_value = "redline,sqlite,postgres")]
    engines: Vec<Engine>,
    #[arg(long, default_value_t = 2_000)]
    accounts: u64,
    #[arg(long, default_value_t = 20_000)]
    events: u64,
    #[arg(long, default_value_t = 200)]
    iters: u64,
    #[arg(long, default_value_t = 40)]
    updates: u64,
    #[arg(long)]
    out: Option<PathBuf>,
    /// Postgres URL. Defaults to `REDLINE_TESTING_POSTGRES_URL`.
    #[arg(long)]
    postgres_url: Option<String>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let scale = Scale {
        accounts: args.accounts,
        events: args.events,
        iters: args.iters,
        updates: args.updates,
    };
    let report = run(&args.engines, &scale, args.postgres_url.as_deref())
        .context("qps comparison failed")?;
    let json = serde_json::to_string_pretty(&report)?;
    if let Some(path) = &args.out {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, &json)?;
    }
    println!("{json}");
    Ok(())
}
