//! Commit and ledger the first recovery unit before starting the kill timer.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::time::Duration;

use anyhow::Result;

use crate::config::{RecoverChildArgs, RunSpec, WorkloadKind};
use crate::engine;

use super::{harness, oracle, units};

pub(super) fn run(args: &RecoverChildArgs) -> Result<()> {
    fs::create_dir_all(&args.db_dir)?;
    let spec = RunSpec {
        engine: args.engine,
        workload: WorkloadKind::SingleRowInsert,
        durability: args.durability,
        threads: 1,
        rows: args.rows,
        duration: Duration::from_secs(1),
        cache_bytes: 8 * 1024 * 1024,
        seed: 7,
        base_dir: args.db_dir.parent().unwrap_or(&args.db_dir).to_path_buf(),
    };
    let engine = engine::open(&spec, &args.db_dir)?;
    engine.setup_schema()?;
    let mut conn = engine.connect(0)?;
    units::ensure_crash_schema(&mut *conn)?;
    // The parent kills this process with SIGKILL, which keeps every
    // completed write(2) in the page cache, so the ledger needs no fsync.
    let mut ack = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.ack_log)?;
    for key in 0..args.rows {
        let digest = units::commit_recovery_unit(
            engine.as_ref(),
            &mut *conn,
            args.scenario,
            key,
            args.rows,
            args.checkpoint_every_rows,
        )?;
        ack.write_all(oracle::ack_line(key as u64, &digest).as_bytes())?;
        // No amount of scheduler or fsync delay before the first commit
        // should turn a healthy crash test into an empty acknowledgement run.
        if key == 0 {
            let mut stdout = std::io::stdout().lock();
            writeln!(stdout, "{}", harness::READY_LINE)?;
            stdout.flush()?;
        }
    }
    Ok(())
}
