//! Row store backing synthetic CTE, view, derived-table and cross-database
//! table defs. The rows belong to the statement whose binding produced them;
//! see `crate::exec::bind_env` (Q5-09).

use std::sync::Arc;

use crate::value::SqlValue;
use redlinedb_kernel::format::RelId;

/// Per-column comparison affinity of a synthetic relation. `None` in the
/// list is a column without affinity.
pub(crate) type ColumnAffinities = Arc<[Option<redlinedb_kernel::catalog::Affinity>]>;

/// Look up the rows backing a synthetic CTE or view TableDef.
pub(crate) fn rows_for_relation(rel: RelId) -> Option<Arc<Vec<Vec<SqlValue>>>> {
    crate::exec::bind_env::rows_for_relation(rel)
}

pub(crate) fn register_cte_rows(rel: RelId, rows: Arc<Vec<Vec<SqlValue>>>) {
    crate::exec::bind_env::register_rows(rel, rows);
}

/// Allow non-CTE callers (e.g. the view module) to publish synthetic
/// row sets into the same store.
pub(crate) fn register_external_rows(rel: RelId, rows: Arc<Vec<Vec<SqlValue>>>) {
    register_cte_rows(rel, rows);
}

/// Remove a synthetic row set by relation id.
pub(crate) fn deregister_rows(rel: RelId) {
    crate::exec::bind_env::deregister_rows(rel);
}

/// Record the column affinities of synthetic relation `rel` (a view's
/// defining expressions, an attached table's declared types). Call after its
/// rows are registered: registering rows drops the relation's record.
pub(crate) fn register_column_affinities(rel: RelId, affinities: ColumnAffinities) {
    crate::exec::bind_env::register_affinities(rel, affinities);
}

/// The column affinities registered for `rel`, if its source recorded them.
pub(crate) fn column_affinities_for_relation(rel: RelId) -> Option<ColumnAffinities> {
    crate::exec::bind_env::affinities_for_relation(rel)
}
