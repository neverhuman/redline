//! RedlineDB through its public Rust API, as an embedding application uses it.

use std::cell::RefCell;
use std::path::Path;

use anyhow::{Context, Result, bail};
use redlinedb::{
    BeginMode, Database, Durability, MemoryOptions, OpenOptions, OwnedStatement, OwnedStep,
    ValueRef,
};

use crate::driver::{Arg, Driver, Rows};
use crate::pair::Pair;

/// Page-cache budget, equal to SQLite's `cache_size=-65536`.
pub const CACHE_BYTES: usize = 64 << 20;

pub struct Redline {
    db: Database,
    conn: RefCell<redlinedb::Connection>,
    pair: Pair,
}

impl Redline {
    /// Open (or create) the database at `path` with the pair's durability,
    /// a 64 MiB page cache and one query thread.
    pub fn open(path: &Path, pair: Pair) -> Result<Self> {
        let options = OpenOptions {
            durability: match pair {
                Pair::Normal => Durability::Normal,
                Pair::Strict => Durability::Strict,
            },
            memory: MemoryOptions {
                cache_bytes: CACHE_BYTES,
            },
            rayon_threads: Some(1),
            ..OpenOptions::default()
        };
        let db = Database::open_with_options(path, options)
            .with_context(|| format!("open {}", path.display()))?;
        let conn = db.connect()?;
        let driver = Self {
            db,
            conn: RefCell::new(conn),
            pair,
        };
        driver.check_settings()?;
        Ok(driver)
    }

    /// Refuse to measure unless the engine runs the pair's durability and
    /// one query thread.
    fn check_settings(&self) -> Result<()> {
        let durability = self.db.commit_durability();
        let wanted = match self.pair {
            Pair::Normal => redlinedb::CommitDurability::Normal,
            Pair::Strict => redlinedb::CommitDurability::Strict,
        };
        if durability != wanted {
            bail!(
                "redline runs {durability:?}, the {} pair needs {wanted:?}",
                self.pair
            );
        }
        // `rayon_threads = Some(1)` disables the query pool, which then
        // reports 0 threads: every query runs on the calling thread.
        if self.db.rayon_thread_count() > 1 {
            bail!(
                "redline runs a {}-thread query pool; the scoreboard is single-threaded",
                self.db.rayon_thread_count()
            );
        }
        Ok(())
    }
}

impl Driver for Redline {
    type Stmt<'a>
        = OwnedStatement
    where
        Self: 'a;

    fn engine_version(&self) -> String {
        format!("redlinedb {}", env!("CARGO_PKG_VERSION"))
    }

    fn prepare(&self, sql: &str) -> Result<OwnedStatement> {
        Ok(self.conn.borrow_mut().prepare_owned(sql)?)
    }

    fn run(stmt: &mut OwnedStatement, args: &[Arg<'_>], columns: usize) -> Result<Rows> {
        stmt.reset()?;
        stmt.clear_bindings();
        for (index, arg) in args.iter().enumerate() {
            match *arg {
                Arg::Int(value) => stmt.bind_i64(index + 1, value)?,
                Arg::Text(value) => stmt.bind_text(index + 1, value)?,
            }
        }
        let mut rows = Rows::default();
        while let OwnedStep::Row = stmt.step()? {
            rows.count += 1;
            for column in 0..columns {
                let part = match stmt.column_ref(column)? {
                    ValueRef::Integer(value) => value,
                    ValueRef::Real(value) => value as i64,
                    ValueRef::Text(text) => text.len() as i64,
                    ValueRef::Blob(blob) => blob.len() as i64,
                    ValueRef::Null => 0,
                };
                rows.digest = rows.digest.wrapping_add(part);
            }
        }
        Ok(rows)
    }

    fn batch(&self, sql: &str) -> Result<()> {
        Ok(self.conn.borrow_mut().execute_batch(sql)?)
    }

    fn begin(&self) -> Result<()> {
        Ok(self.conn.borrow_mut().begin(BeginMode::Immediate)?)
    }

    fn commit(&self) -> Result<()> {
        self.conn.borrow_mut().commit()?;
        Ok(())
    }

    fn checkpoint(&self) -> Result<()> {
        self.db.checkpoint()?;
        Ok(())
    }

    fn settings(&self) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "durability": format!("{:?}", self.db.commit_durability()),
            "cache_bytes": CACHE_BYTES,
            "buffer_pool_pages": self.db.buffer_pool_pages(),
            "query_pool_threads": self.db.rayon_thread_count(),
        }))
    }

    fn counters(&self) -> Option<serde_json::Value> {
        serde_json::to_value(redlinedb_kernel::observe::snapshot()).ok()
    }
}
