use redlinedb_kernel::Error as KernelError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("kernel error: {0}")]
    Kernel(#[source] KernelError),

    #[error("parse error: {0}")]
    Parse(String),

    #[error("unsupported sql: {0}")]
    UnsupportedSql(String),

    #[error("bind error: {0}")]
    Bind(String),

    #[error("unknown table: {0}")]
    UnknownTable(String),

    #[error("unknown column: {0}")]
    UnknownColumn(String),

    #[error("ambiguous column: {0}")]
    AmbiguousColumn(String),

    #[error("parameter out of range: {0}")]
    ParameterOutOfRange(usize),

    #[error("schema changed")]
    SchemaChanged,

    #[error("transaction state error: {0}")]
    TransactionState(&'static str),

    #[error("commit outcome uncertain")]
    CommitMaybeCommitted,

    #[error("constraint violation: {0}")]
    ConstraintViolation(String),

    #[error("datatype mismatch")]
    DatatypeMismatch,

    #[error("configuration error: {0}")]
    Config(String),

    #[error("not authorized")]
    NotAuthorized,

    #[error("trigger ignored row")]
    TriggerIgnore,

    /// SQLite's `integer overflow`: `abs(-9223372036854775808)`, or a
    /// `sum()` whose all-INTEGER running total left the i64 range.
    #[error("integer overflow")]
    IntegerOverflow,

    /// Postgres-dialect INTEGER arithmetic that leaves the i64 range, where
    /// SQLite would answer REAL.
    #[error("bigint out of range")]
    BigintOutOfRange,

    /// A statement the parser accepts but the engine cannot honour
    /// faithfully, refused instead of answered with a stand-in.
    #[error("unsupported capability: {feature}: {detail}")]
    UnsupportedCapability {
        feature: &'static str,
        detail: String,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Kernel(a), Self::Kernel(b)) => a == b,
            (Self::Parse(a), Self::Parse(b)) => a == b,
            (Self::UnsupportedSql(a), Self::UnsupportedSql(b)) => a == b,
            (Self::Bind(a), Self::Bind(b)) => a == b,
            (Self::UnknownTable(a), Self::UnknownTable(b)) => a == b,
            (Self::UnknownColumn(a), Self::UnknownColumn(b)) => a == b,
            (Self::AmbiguousColumn(a), Self::AmbiguousColumn(b)) => a == b,
            (Self::ParameterOutOfRange(a), Self::ParameterOutOfRange(b)) => a == b,
            (Self::SchemaChanged, Self::SchemaChanged) => true,
            (Self::TransactionState(a), Self::TransactionState(b)) => a == b,
            (Self::CommitMaybeCommitted, Self::CommitMaybeCommitted) => true,
            (Self::ConstraintViolation(a), Self::ConstraintViolation(b)) => a == b,
            (Self::DatatypeMismatch, Self::DatatypeMismatch) => true,
            (Self::Config(a), Self::Config(b)) => a == b,
            (Self::NotAuthorized, Self::NotAuthorized) => true,
            (Self::TriggerIgnore, Self::TriggerIgnore) => true,
            (Self::IntegerOverflow, Self::IntegerOverflow) => true,
            (Self::BigintOutOfRange, Self::BigintOutOfRange) => true,
            (
                Self::UnsupportedCapability {
                    feature: fa,
                    detail: da,
                },
                Self::UnsupportedCapability {
                    feature: fb,
                    detail: db,
                },
            ) => fa == fb && da == db,
            _ => false,
        }
    }
}

impl Eq for Error {}

impl From<KernelError> for Error {
    fn from(err: KernelError) -> Self {
        match err {
            // The commit record was queued before the WAL failed, so the
            // next open may find the transaction committed. That is the
            // same promise `CommitMaybeCommitted` makes; do not report it
            // as a failure the caller may simply retry.
            KernelError::CommitOutcomeUnknown { .. } => Self::CommitMaybeCommitted,
            other => Self::Kernel(other),
        }
    }
}

impl From<sqlparser::parser::ParserError> for Error {
    fn from(value: sqlparser::parser::ParserError) -> Self {
        Self::Parse(value.to_string())
    }
}

impl From<&'static str> for Error {
    fn from(value: &'static str) -> Self {
        Self::UnsupportedSql(value.to_owned())
    }
}
