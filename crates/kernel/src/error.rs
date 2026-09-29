use std::io;

use redlinedb_domain::DomainError;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

/// Common fixes printed in [`DomainError::common_fixes`] when an
/// [`Error::InvalidChecksum`] is escalated through the domain surface.
///
/// Kept as a `const` (rather than constructed per call) so the
/// `&'static [&'static str]` field on [`DomainError`] points at a
/// stable rodata slice the next agent can grep for.
const INVALID_CHECKSUM_FIXES: &[&str] = &[
    "rerun `integrity::verify` on the affected page file",
    "check `.jankurai/proof-lanes.toml` for the `phase9-recovery-matrix` lane",
    "inspect the WAL tail with `cargo run -p redlinedb-cli -- wal-dump`",
];

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    #[error("invalid checksum")]
    InvalidChecksum,

    #[error("invalid magic: expected {expected:#x}, got {actual:#x}")]
    InvalidMagic { expected: u32, actual: u32 },

    #[error("unsupported format version: {0}")]
    UnsupportedVersion(u16),

    #[error("buffer too small: need {needed} bytes, got {actual}")]
    BufferTooSmall { needed: usize, actual: usize },

    #[error("corrupt page: {0}")]
    CorruptPage(&'static str),

    #[error("corrupt wal: {0}")]
    CorruptWal(&'static str),

    /// The WAL writer thread hit an I/O error and stopped. Every later
    /// append and every wait for a record past `at_lsn` fails with this.
    #[error("wal writer failed during {stage} at lsn {}: {kind}", .at_lsn.0)]
    WalWriterFailed {
        stage: crate::wal::WalFailureStage,
        kind: io::ErrorKind,
        at_lsn: crate::format::Lsn,
    },

    /// The commit record was queued, then the WAL failed before it was known
    /// to be written (Normal) or durable (Strict). The transaction is not
    /// visible in this process, but the record may be in the WAL, and then
    /// the next open recovers it as committed. Do not treat this as a
    /// rollback: check for the transaction's effects after reopening before
    /// retrying a change that must happen only once.
    #[error(
        "commit outcome unknown for transaction {} (commit record ends at lsn {}): {cause}",
        .tx_id.0,
        .end_lsn.0
    )]
    CommitOutcomeUnknown {
        tx_id: crate::format::TxId,
        end_lsn: crate::format::Lsn,
        #[source]
        cause: Box<Error>,
    },

    #[error("no free slot space on page")]
    PageFull,

    #[error("record needs {needed} bytes but one heap page can hold at most {maximum}")]
    RecordTooLarge { needed: usize, maximum: usize },

    #[error("transaction is not visible in this snapshot")]
    NotVisible,

    #[error("write conflict")]
    WriteConflict,

    /// An insert asked for a row id that a row still holds: one committed,
    /// or written by the inserting transaction itself.
    #[error("row id is in use")]
    RowIdInUse,

    #[error("lock timeout")]
    LockTimeout,

    #[error("serialization failure")]
    SerializationFailure,

    #[error("unsupported isolation level")]
    UnsupportedIsolation,

    #[error("transaction is already closed")]
    TransactionClosed,

    #[error("catalog corruption: {0}")]
    CatalogCorrupt(&'static str),

    #[error("object already exists")]
    ObjectExists,

    #[error("object not found")]
    ObjectNotFound,

    #[error("column not found")]
    ColumnNotFound,

    #[error("schema changed")]
    SchemaChanged,

    #[error("constraint violation: {0}")]
    ConstraintViolation(&'static str),

    #[error("datatype mismatch")]
    DatatypeMismatch,

    #[error("unsupported ddl: {0}")]
    UnsupportedDdl(&'static str),

    #[error("invalid record: {0}")]
    InvalidRecord(&'static str),

    #[error("invalid jsonb: {0}")]
    InvalidJsonb(&'static str),

    #[error("invalid json path: {0}")]
    InvalidJsonPath(&'static str),

    #[error("vector error: {0}")]
    Vector(String),
}

impl From<crate::vector::VectorError> for Error {
    fn from(err: crate::vector::VectorError) -> Self {
        Error::Vector(err.to_string())
    }
}

impl Error {
    /// Escalate this kernel error into a [`DomainError`] carrying agent
    /// context (purpose, repair hint, docs URL).
    ///
    /// Today this is only implemented for [`Error::InvalidChecksum`] —
    /// the canonical "next agent must run a proof lane" failure — and
    /// returns `None` for other variants so callers can opportunistically
    /// upgrade without losing fidelity for kernel-internal handling.
    ///
    /// See `docs/audit-rubric.md` for the dimension-to-evidence mapping
    /// and `crates/domain/src/error.rs` for the typed exception contract.
    pub fn into_domain(self) -> Option<DomainError> {
        match self {
            Error::InvalidChecksum => Some(
                DomainError::new(
                    "kernel.storage.invalid_checksum",
                    "a page or WAL frame failed checksum verification",
                    INVALID_CHECKSUM_FIXES,
                    "docs/testing.md#proof-lanes",
                    "rerun `just fast`; if it persists, run \
                     the `phase9-recovery-matrix` lane and capture \
                     the failing page id from `.jankurai/proof-receipt-template.md`",
                )
                .with_source(self),
            ),
            _ => None,
        }
    }
}

impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Io(left), Self::Io(right)) => left.kind() == right.kind(),
            (Self::InvalidChecksum, Self::InvalidChecksum) => true,
            (
                Self::InvalidMagic {
                    expected: left_expected,
                    actual: left_actual,
                },
                Self::InvalidMagic {
                    expected: right_expected,
                    actual: right_actual,
                },
            ) => left_expected == right_expected && left_actual == right_actual,
            (Self::UnsupportedVersion(left), Self::UnsupportedVersion(right)) => left == right,
            (
                Self::BufferTooSmall {
                    needed: left_needed,
                    actual: left_actual,
                },
                Self::BufferTooSmall {
                    needed: right_needed,
                    actual: right_actual,
                },
            ) => left_needed == right_needed && left_actual == right_actual,
            (Self::CorruptPage(left), Self::CorruptPage(right)) => left == right,
            (Self::CorruptWal(left), Self::CorruptWal(right)) => left == right,
            (
                Self::WalWriterFailed {
                    stage: left_stage,
                    kind: left_kind,
                    at_lsn: left_lsn,
                },
                Self::WalWriterFailed {
                    stage: right_stage,
                    kind: right_kind,
                    at_lsn: right_lsn,
                },
            ) => left_stage == right_stage && left_kind == right_kind && left_lsn == right_lsn,
            (
                Self::CommitOutcomeUnknown {
                    tx_id: left_tx,
                    end_lsn: left_lsn,
                    cause: left_cause,
                },
                Self::CommitOutcomeUnknown {
                    tx_id: right_tx,
                    end_lsn: right_lsn,
                    cause: right_cause,
                },
            ) => left_tx == right_tx && left_lsn == right_lsn && left_cause == right_cause,
            (Self::PageFull, Self::PageFull) => true,
            (
                Self::RecordTooLarge {
                    needed: left_needed,
                    maximum: left_maximum,
                },
                Self::RecordTooLarge {
                    needed: right_needed,
                    maximum: right_maximum,
                },
            ) => left_needed == right_needed && left_maximum == right_maximum,
            (Self::NotVisible, Self::NotVisible) => true,
            (Self::WriteConflict, Self::WriteConflict) => true,
            (Self::LockTimeout, Self::LockTimeout) => true,
            (Self::SerializationFailure, Self::SerializationFailure) => true,
            (Self::UnsupportedIsolation, Self::UnsupportedIsolation) => true,
            (Self::TransactionClosed, Self::TransactionClosed) => true,
            (Self::CatalogCorrupt(left), Self::CatalogCorrupt(right)) => left == right,
            (Self::ObjectExists, Self::ObjectExists) => true,
            (Self::ObjectNotFound, Self::ObjectNotFound) => true,
            (Self::ColumnNotFound, Self::ColumnNotFound) => true,
            (Self::SchemaChanged, Self::SchemaChanged) => true,
            (Self::ConstraintViolation(left), Self::ConstraintViolation(right)) => left == right,
            (Self::DatatypeMismatch, Self::DatatypeMismatch) => true,
            (Self::UnsupportedDdl(left), Self::UnsupportedDdl(right)) => left == right,
            (Self::InvalidRecord(left), Self::InvalidRecord(right)) => left == right,
            (Self::InvalidJsonb(left), Self::InvalidJsonb(right)) => left == right,
            (Self::InvalidJsonPath(left), Self::InvalidJsonPath(right)) => left == right,
            (Self::Vector(left), Self::Vector(right)) => left == right,
            _ => false,
        }
    }
}

impl Eq for Error {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn invalid_checksum_into_domain_populates_agent_context() {
        let domain = Error::InvalidChecksum
            .into_domain()
            .expect("InvalidChecksum must escalate to DomainError");

        assert_eq!(domain.purpose, "kernel.storage.invalid_checksum");
        assert_eq!(
            domain.reason,
            "a page or WAL frame failed checksum verification"
        );
        assert_eq!(domain.common_fixes, INVALID_CHECKSUM_FIXES);
        assert_eq!(domain.docs_url, "docs/testing.md#proof-lanes");
        assert!(domain.repair_hint.contains("phase9-recovery-matrix"));

        let source = domain
            .source()
            .expect("DomainError must carry the original kernel error as source");
        assert_eq!(source.to_string(), "invalid checksum");
    }

    #[test]
    fn non_escalated_variants_return_none() {
        assert!(Error::PageFull.into_domain().is_none());
        assert!(Error::NotVisible.into_domain().is_none());
    }
}
