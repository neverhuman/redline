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
/// read (integers summed, text by length) that both engines must agree on.
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
}

/// An engine under test. Statements are prepared once and stepped many
/// times, as an application that caches its statements would.
pub trait Driver {
    type Stmt<'a>
    where
        Self: 'a;

    /// The engine's name and version as it reports them.
    fn engine_version(&self) -> String;

    fn prepare(&self, sql: &str) -> Result<Self::Stmt<'_>>;

    /// Bind `args`, step every row, and read the first `columns` columns
    /// of each (0 reads none; the rows are still produced).
    fn run(stmt: &mut Self::Stmt<'_>, args: &[Arg<'_>], columns: usize) -> Result<Rows>;

    /// Run a script of one or more statements that return no rows.
    fn batch(&self, sql: &str) -> Result<()>;

    fn begin(&self) -> Result<()>;
    fn commit(&self) -> Result<()>;

    /// Make every change durable and fold the log into the database file,
    /// so a copied image carries no log tail.
    fn checkpoint(&self) -> Result<()>;

    /// The settings the engine actually runs with, read back from it.
    fn settings(&self) -> Result<serde_json::Value>;

    /// Engine counters, where the engine has them.
    fn counters(&self) -> Option<serde_json::Value>;
}

/// Run `sql` once with `args` and return its first column, summed over rows.
pub fn one<D: Driver>(driver: &D, sql: &str, args: &[Arg<'_>]) -> Result<i64> {
    let mut stmt = driver.prepare(sql)?;
    Ok(D::run(&mut stmt, args, 1)?.digest)
}
