//! One interface over both engines, so every workload is written once and
//! runs the same statements, parameters and row reads on each.

use anyhow::Result;

/// A bound parameter.
#[derive(Clone, Copy, Debug)]
pub enum Arg<'a> {
    Int(i64),
    Text(&'a str),
}

/// What a statement returned: its row count, and a digest of the columns
/// read that both engines must agree on. The digest adds one hash per row,
/// so it does not depend on row order (GROUP BY output has none), but a
/// value paired with the wrong key changes it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rows {
    pub count: u64,
    pub digest: i64,
}

impl Rows {
    pub fn add(&mut self, other: Rows) {
        self.count += other.count;
        self.digest = self.digest.wrapping_add(other.digest);
    }

    /// Fold one row, given its column values as integers.
    pub fn push_row(&mut self, values: impl IntoIterator<Item = i64>) {
        let mut hash: i64 = 0x5bd1_e995;
        for value in values {
            hash = hash.wrapping_mul(1_000_003) ^ value;
        }
        self.count += 1;
        self.digest = self.digest.wrapping_add(hash);
    }
}

/// An engine under test. Statements are prepared once and stepped many
/// times, as an application that caches its statements would.
pub trait Driver {
    type Stmt<'a>
    where
        Self: 'a;

    /// The engine's name and version as it reports them.
    fn engine_version(&self) -> String;

    /// Refuse to measure unless the engine runs with its pair's settings.
    /// Called once the database is open and, for an open workload, after
    /// the clock has stopped.
    fn verify(&self) -> Result<()>;

    fn prepare(&self, sql: &str) -> Result<Self::Stmt<'_>>;

    /// Bind `args`, step every row, and read the first `columns` columns
    /// of each (0 reads none; the rows are still produced and counted).
    fn run(stmt: &mut Self::Stmt<'_>, args: &[Arg<'_>], columns: usize) -> Result<Rows>;

    /// Run a script of one or more statements that return no rows.
    fn batch(&self, sql: &str) -> Result<()>;

    fn begin(&self) -> Result<()>;
    fn commit(&self) -> Result<()>;

    /// Make every change durable and fold the log into the database file,
    /// so a copied image carries no log tail.
    fn checkpoint(&self) -> Result<()>;

    /// Close without folding the log into the database file, so the next
    /// open finds the log as a crash or a busy writer would leave it.
    fn close_leaving_log(self) -> Result<()>;

    /// The settings the engine actually runs with, read back from it.
    fn settings(&self) -> Result<serde_json::Value>;

    /// Process-wide engine counters, where the engine has them.
    fn counters() -> Option<serde_json::Value>;
}

/// Run `sql` once with `args` and return its first column, summed over rows.
pub fn one<D: Driver>(driver: &D, sql: &str, args: &[Arg<'_>]) -> Result<Rows> {
    let mut stmt = driver.prepare(sql)?;
    D::run(&mut stmt, args, 1)
}
