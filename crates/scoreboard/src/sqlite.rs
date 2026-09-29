//! SQLite through rusqlite, configured to match RedlineDB's pair.

use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, Statement};

use crate::driver::{Arg, Driver, Rows};
use crate::pair::Pair;

pub struct Sqlite {
    conn: Connection,
    pair: Pair,
}

impl Sqlite {
    /// Open (or create) the database at `path`, set as RedlineDB runs:
    /// - one process owns the file (`locking_mode=EXCLUSIVE`, as RedlineDB's
    ///   owner lock), so no file lock is taken per statement;
    /// - WAL, with the pair's `synchronous`;
    /// - a 64 MiB cache, 4 KiB pages, no mmap, temp storage in memory;
    /// - foreign keys off, RedlineDB's default.
    ///
    /// `locking_mode` goes first so WAL never uses shared memory, and
    /// `page_size` only takes effect on an empty file. Settings are checked
    /// by [`Driver::verify`], outside any timed open.
    pub fn open(path: &Path, pair: Pair) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        conn.pragma_update(None, "locking_mode", "EXCLUSIVE")?;
        conn.pragma_update(None, "page_size", 4096)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", pair.sqlite_synchronous())?;
        conn.pragma_update(None, "cache_size", -65536)?;
        conn.pragma_update(None, "mmap_size", 0)?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        conn.pragma_update(None, "foreign_keys", "OFF")?;
        Ok(Self { conn, pair })
    }

    /// Stop SQLite folding the log into the database file every 1000
    /// pages, so the updates of the after-updates image stay in the log.
    pub fn keep_log(&self) -> Result<()> {
        self.conn.pragma_update(None, "wal_autocheckpoint", 0)?;
        Ok(())
    }

    fn pragma(&self, name: &str) -> Result<serde_json::Value> {
        let sql = format!("PRAGMA {name}"); // jankurai:allow HLT-023-INPUT-BOUNDARY-GAP reason=pragma-name-from-a-fixed-internal-list-not-input expires=2027-09-30
        let value = self.conn.query_row(&sql, [], |row| {
            Ok(match row.get_ref(0)? {
                ValueRef::Integer(value) => serde_json::json!(value),
                ValueRef::Text(text) => {
                    serde_json::json!(String::from_utf8_lossy(text).to_ascii_lowercase())
                }
                other => serde_json::json!(format!("{other:?}")),
            })
        })?;
        Ok(value)
    }
}

impl Driver for Sqlite {
    type Stmt<'a>
        = Statement<'a>
    where
        Self: 'a;

    fn engine_version(&self) -> String {
        format!("sqlite {}", rusqlite::version())
    }

    fn verify(&self) -> Result<()> {
        let settings = self.settings()?;
        let expect = [
            ("locking_mode", serde_json::json!("exclusive")),
            ("journal_mode", serde_json::json!("wal")),
            (
                "synchronous",
                serde_json::json!(match self.pair {
                    Pair::Normal => 1,
                    Pair::Strict => 2,
                }),
            ),
            ("cache_size", serde_json::json!(-65536)),
            ("mmap_size", serde_json::json!(0)),
            ("page_size", serde_json::json!(4096)),
            ("temp_store", serde_json::json!(2)),
            ("foreign_keys", serde_json::json!(0)),
        ];
        for (name, wanted) in expect {
            if settings[name] != wanted {
                bail!(
                    "sqlite {name} reads back {} but the {} pair needs {wanted}",
                    settings[name],
                    self.pair
                );
            }
        }
        Ok(())
    }

    fn prepare(&self, sql: &str) -> Result<Statement<'_>> {
        Ok(self.conn.prepare(sql)?)
    }

    fn run(stmt: &mut Statement<'_>, args: &[Arg<'_>], columns: usize) -> Result<Rows> {
        for (index, arg) in args.iter().enumerate() {
            match *arg {
                Arg::Int(value) => stmt.raw_bind_parameter(index + 1, value)?,
                Arg::Text(value) => stmt.raw_bind_parameter(index + 1, value)?,
            }
        }
        let mut out = Rows::default();
        let mut values = Vec::with_capacity(columns);
        let mut rows = stmt.raw_query();
        while let Some(row) = rows.next()? {
            values.clear();
            for column in 0..columns {
                values.push(match row.get_ref(column)? {
                    ValueRef::Integer(value) => value,
                    ValueRef::Real(value) => value as i64,
                    ValueRef::Text(text) => text.len() as i64,
                    ValueRef::Blob(blob) => blob.len() as i64,
                    ValueRef::Null => 0,
                });
            }
            out.push_row(values.iter().copied());
        }
        Ok(out)
    }

    fn batch(&self, sql: &str) -> Result<()> {
        Ok(self.conn.execute_batch(sql)?)
    }

    fn begin(&self) -> Result<()> {
        Ok(self.conn.execute_batch("BEGIN IMMEDIATE")?)
    }

    fn commit(&self) -> Result<()> {
        Ok(self.conn.execute_batch("COMMIT")?)
    }

    fn checkpoint(&self) -> Result<()> {
        self.conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }

    fn close_leaving_log(self) -> Result<()> {
        // Closing the last connection folds the log into the file. Leaking
        // the handle instead leaves the log for the next open to recover,
        // as a process that exits without closing does.
        std::mem::forget(self.conn);
        Ok(())
    }

    fn settings(&self) -> Result<serde_json::Value> {
        let mut settings = serde_json::Map::new();
        for name in [
            "locking_mode",
            "journal_mode",
            "synchronous",
            "cache_size",
            "mmap_size",
            "page_size",
            "temp_store",
            "foreign_keys",
        ] {
            settings.insert(name.to_owned(), self.pragma(name)?);
        }
        settings.insert("version".to_owned(), serde_json::json!(rusqlite::version()));
        Ok(serde_json::Value::Object(settings))
    }

    fn counters() -> Option<serde_json::Value> {
        None
    }
}
