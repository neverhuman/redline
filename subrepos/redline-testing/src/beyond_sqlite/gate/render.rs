//! The README block (`<!-- POSTGRES_PARITY_START -->`) the gate writes.
//!
//! Every word of it is here, so the README cannot say more than the
//! qualification report does: the split between row matches and expected
//! rejections, refusals and mismatches, what the comparison is, and what the
//! corpus does not cover. The gate writes it only from release evidence
//! (a clean source tree at a recorded commit, a measured reference), so it
//! carries no "dirty" caveat.
use super::Qualification;

/// Surfaces the SQL-shell corpus cannot establish; the capability matrix
/// (`metadata/beyond_sqlite/postgres-capabilities.json`) gives each a status.
pub(super) const NOT_COVERED: [&str; 7] = [
    "wire protocol",
    "TLS",
    "roles/authorization",
    "SQLSTATE",
    "NOTIFY delivery",
    "replication/CDC",
    "extensions",
];

pub(super) fn markdown(report: &Qualification) -> String {
    format!(
        "PostgreSQL **16.15** SQL-shell corpus (`redlinedb` CLI, `REDLINEDB_RESULT_DIALECT=postgres`, fresh `:memory:` per case): **{passed}/{required} agree** = **{positive}** row matches + **{rejections}** expected rejections (declared error text verified); **{unsupported}** declared unsupported; **{mismatches}** mismatches; **{skipped}** skipped.\n\nAgreement is {comparison}, not typed-result or application parity. Not covered: {not_covered} ([capability matrix](docs/beyond-postgres-skips.md#capability-matrix)). Source `{source}`; corpus SHA-256 `{corpus}`.\n",
        passed = report.passed,
        required = report.required,
        positive = report.positive_matches,
        rejections = report.expected_rejections.len(),
        unsupported = report.declared_unsupported.len(),
        mismatches = report.mismatches.len(),
        skipped = report.skipped,
        comparison = report.comparison,
        not_covered = NOT_COVERED.join(", "),
        source = report.source_commit.as_str().unwrap_or("unrecorded"),
        corpus = report.corpus_sha256,
    )
}
