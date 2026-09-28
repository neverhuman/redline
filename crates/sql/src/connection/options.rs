use std::path::PathBuf;
use std::time::Duration;

use redlinedb_kernel::engine::EngineConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbOptions {
    pub engine: EngineConfig,
    pub unique_lock_shards: usize,
    pub statement_cache_capacity: usize,
    pub busy_timeout: Duration,
    pub optimizer: OptimizerConfig,
    pub query_memory: QueryMemoryConfig,
    pub stats: StatsConfig,
    pub temp_dir: Option<PathBuf>,
    /// SQL dialect of every connection to this database. `None` reads
    /// [`Dialect::ENV_VAR`] once, when the database is created or opened;
    /// later changes to the environment do not affect an open database.
    pub dialect: Option<Dialect>,
}

/// The SQL dialect a database's connections speak.
///
/// `Sqlite` is the default. `PostgresSubset` turns on the Postgres-shaped
/// rewrites, casts and result rendering the beyond-SQLite corpus exercises
/// (booleans as `t`/`f`, schema-qualified names, enums, domains, `::citext`).
/// The dialect is fixed per database when it is opened, and every connection
/// to it, including a statement's worker threads, sees the same value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Dialect {
    #[default]
    Sqlite,
    PostgresSubset,
}

impl Dialect {
    /// The environment variable read when [`DbOptions::dialect`] is `None`.
    /// `postgres` selects [`Dialect::PostgresSubset`]; anything else, or no
    /// value, selects [`Dialect::Sqlite`].
    pub const ENV_VAR: &'static str = "REDLINEDB_RESULT_DIALECT";

    /// The dialect [`Dialect::ENV_VAR`] names right now. Embedders and the
    /// CLI call this once at startup; the engine never reads the variable
    /// while it evaluates a statement.
    pub fn from_env() -> Self {
        match std::env::var(Self::ENV_VAR).ok().as_deref() {
            Some("postgres") => Self::PostgresSubset,
            _ => Self::Sqlite,
        }
    }

    pub fn is_postgres(self) -> bool {
        matches!(self, Self::PostgresSubset)
    }
}

impl Default for DbOptions {
    fn default() -> Self {
        Self {
            engine: EngineConfig::default(),
            unique_lock_shards: 128,
            statement_cache_capacity: 128,
            busy_timeout: Duration::from_secs(5),
            optimizer: OptimizerConfig::default(),
            query_memory: QueryMemoryConfig::default(),
            stats: StatsConfig::default(),
            temp_dir: None,
            dialect: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OptimizerConfig {
    pub enabled: bool,
    pub max_exact_join_tables: usize,
    pub max_join_alternatives: usize,
    pub enable_multi_index_or: bool,
    pub enable_multi_index_and: bool,
    pub enable_covering_index: bool,
}

impl Default for OptimizerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_exact_join_tables: 8,
            max_join_alternatives: 4,
            enable_multi_index_or: true,
            enable_multi_index_and: true,
            enable_covering_index: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct QueryMemoryConfig {
    pub work_mem_bytes: usize,
    pub max_spill_bytes: usize,
    pub batch_rows: usize,
}

impl Default for QueryMemoryConfig {
    fn default() -> Self {
        Self {
            work_mem_bytes: 8 * 1024 * 1024,
            max_spill_bytes: 1024 * 1024 * 1024,
            batch_rows: 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StatsConfig {
    pub exact_analyze_row_threshold: usize,
    pub sample_rows: usize,
    pub mcv_capacity: usize,
    pub histogram_buckets: usize,
}

impl Default for StatsConfig {
    fn default() -> Self {
        Self {
            exact_analyze_row_threshold: 100_000,
            sample_rows: 32_768,
            mcv_capacity: 100,
            histogram_buckets: 100,
        }
    }
}
