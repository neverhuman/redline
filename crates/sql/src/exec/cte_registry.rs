//! Row store backing synthetic CTE, view, derived-table and cross-database
//! table defs. The rows belong to the statement whose binding produced them;
//! see `crate::exec::bind_env` (Q5-09).

use std::sync::Arc;

use crate::value::SqlValue;
use redlinedb_kernel::format::RelId;

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
