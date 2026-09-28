//! The open-time upgrade for key collations (workplan Q5-10, step 4).
//!
//! A database written before index keys inherited their column's declared
//! NOCASE or RTRIM has indexes on such columns that compare BINARY. At open
//! every one of them is brought in line in the same transaction as the
//! index-format upgrade in `reindex.rs`: an index with
//! a B-tree is rebuilt (the kernel lists it in `indexes_needing_rebuild`,
//! and every rebuild applies the inherited collations); a UNIQUE or PRIMARY
//! KEY constraint without a B-tree only gets its catalog entry changed,
//! after this module has checked that no two rows now share a key. Two
//! rows that do fail the open, naming the index; nothing is committed.

use std::collections::HashSet;

use redlinedb_kernel::catalog::collation::index_keys_needing_inherited_collation;
use redlinedb_kernel::catalog::{IndexId, SchemaSnapshot};
use redlinedb_kernel::engine::{Engine, Txn};

use crate::connection::Connection;
use crate::error::{Error, Result};

use super::{collect_table_rows, index_dml, index_predicate};

/// Indexes without a B-tree whose keys should inherit a declared collation.
/// (Those with a B-tree are rebuilt instead.)
pub(crate) fn constraint_indexes_to_recollate(engine: &Engine) -> Vec<IndexId> {
    let snapshot = engine.schema_snapshot();
    engine
        .indexes_needing_inherited_collation()
        .into_iter()
        .filter(|id| {
            snapshot
                .index_by_id(*id)
                .is_some_and(|index| index.meta_page_id.is_none())
        })
        .collect()
}

/// Give index `index_id` (one without a B-tree) its inherited collations
/// in `tx`, failing when two of its table's rows now share a key.
pub(crate) fn recollate_constraint_index(
    conn: &Connection,
    tx: &mut Txn,
    index_id: IndexId,
) -> Result<()> {
    let index = conn.engine().inherit_index_key_collations(tx, index_id)?;
    if !index.unique {
        return Ok(());
    }
    let snapshot = conn.engine().schema_snapshot_for_tx(tx);
    let table = snapshot
        .table_by_id(index.table_id)
        .ok_or(redlinedb_kernel::Error::ObjectNotFound)?;
    let mut seen: HashSet<Vec<u8>> = HashSet::new();
    for row in collect_table_rows(conn.engine(), tx, &table)? {
        if let Some(predicate) = index.predicate_sql.as_deref()
            && !index_predicate::eval_index_predicate(&table, predicate, &row.values)?
        {
            continue;
        }
        let key = index_dml::build_index_key(&table, &index, &row.values)?;
        if !key.contains_null && !seen.insert(key.bytes) {
            return Err(Error::ConstraintViolation(format!(
                "UNIQUE constraint failed: {}",
                table.name
            )));
        }
    }
    Ok(())
}

/// Whether `index_id` was upgraded (in part) to inherit a declared
/// collation, judged on the snapshot from before the upgrade.
pub(crate) fn inherits_collation(snapshot: &SchemaSnapshot, index_id: IndexId) -> bool {
    snapshot.index_by_id(index_id).is_some_and(|index| {
        snapshot
            .table_by_id(index.table_id)
            .is_some_and(|table| index_keys_needing_inherited_collation(&table, &index).is_some())
    })
}

/// The part of an upgrade failure message that explains a collation
/// duplicate.
pub(crate) const COLLATION_DUPLICATE: &str = "this version gives the index the NOCASE or RTRIM \
     collation its column declares, under which values such as 'x' and 'X' (NOCASE) or 'x' and \
     'x ' (RTRIM) are one key";
