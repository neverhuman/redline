//! `REINDEX` and the open-time index-format upgrade.
//!
//! Both rebuild an index the same way: `Engine::rebuild_index_empty` gives it
//! a new, empty B-tree at the current index-format epoch inside the
//! transaction, and [`backfill_sql_index`] fills it from the heap with the
//! key builder DML maintenance uses, evaluating expression keys and
//! partial-index predicates. The catalog switches to the new B-tree only at
//! COMMIT (see `crates/kernel/src/engine/catalog_ops/index_rebuild.rs`), so a
//! failed or interrupted rebuild leaves the old index in place.

use std::sync::Arc;

use redlinedb_kernel::catalog::{IndexDef, IndexId, IndexKeySource, SchemaSnapshot};
use redlinedb_kernel::engine::{CommitOutcome, Txn};
use redlinedb_kernel::index::INDEX_VERSION;
use redlinedb_kernel::txn::Isolation;

use crate::connection::{Connection, Database};
use crate::error::{Error, Result};
use crate::session::SessionState;
use crate::statement::ReindexTarget;

use super::{collect_table_rows, index_dml, index_predicate, with_write_tx};

/// Fill `index`'s B-tree (as `tx` sees it) from every row of its table that
/// `tx` sees. Used by CREATE INDEX for expression and partial indexes and by
/// every rebuild. A UNIQUE index whose rows share a key fails with a
/// `UNIQUE constraint failed` error.
pub(crate) fn backfill_sql_index(
    conn: &Connection,
    tx: &mut Txn,
    index: &Arc<IndexDef>,
) -> Result<()> {
    let Some(handle) = index_dml::open_index_handle_for_tx(conn.engine(), tx, index) else {
        return Ok(());
    };
    let snapshot = conn.engine().schema_snapshot_for_tx(tx);
    let table = snapshot
        .table_by_id(index.table_id)
        .ok_or(redlinedb_kernel::Error::ObjectNotFound)?;
    for row in collect_table_rows(conn.engine(), tx, &table)? {
        if let Some(pred_sql) = index.predicate_sql.as_deref()
            && !index_predicate::eval_index_predicate(&table, pred_sql, &row.values)?
        {
            continue;
        }
        let key = index_dml::build_index_key(&table, index, &row.values)?;
        let _unique_guard = if index.unique && !key.contains_null {
            let (guard, hit) =
                index_dml::probe_unique_for_conflict(conn.engine(), &handle, tx, None, &key)?;
            if hit.is_some() {
                return Err(Error::ConstraintViolation(format!(
                    "UNIQUE constraint failed: {}",
                    table.name
                )));
            }
            Some(guard)
        } else {
            None
        };
        handle.insert_tx(tx.id(), &key.bytes, index_dml::synthetic_row_ref(row.rowid))?;
    }
    Ok(())
}

/// Rebuild index `index_id` from the heap inside `tx`.
fn rebuild_index(conn: &Connection, tx: &mut Txn, index_id: IndexId) -> Result<()> {
    let index = conn.engine().rebuild_index_empty(tx, index_id)?;
    backfill_sql_index(conn, tx, &index)
}

/// Run `REINDEX target` in the connection's write transaction.
pub(crate) fn execute_reindex(conn: &Connection, target: &ReindexTarget) -> Result<()> {
    with_write_tx(conn, |session, tx| {
        let snapshot = conn.engine().schema_snapshot_for_tx(tx);
        for index_id in resolve_target(conn, session, &snapshot, target)? {
            rebuild_index(conn, tx, index_id)?;
        }
        Ok(())
    })
}

/// Rebuild, in one transaction, every index whose B-tree is at an older
/// index-format epoch, so the database is only handed out once every index
/// uses the current key format. On any failure nothing is committed: the
/// indexes stay at their old epoch and the error names the index.
pub(crate) fn upgrade_stale_indexes(db: &Arc<Database>) -> Result<()> {
    let conn = db.connect();
    let engine = Arc::clone(conn.engine());
    let stale = engine.indexes_needing_rebuild()?;
    if stale.is_empty() {
        return Ok(());
    }
    let mut tx = engine.begin(Isolation::Snapshot)?;
    for &index_id in &stale {
        if let Err(err) = rebuild_index(&conn, &mut tx, index_id) {
            let described = describe_upgrade_failure(&engine.schema_snapshot(), index_id, err);
            engine.rollback(tx)?;
            return Err(described);
        }
    }
    match engine.commit(tx)? {
        CommitOutcome::Committed(_) => Ok(()),
        CommitOutcome::MaybeCommitted => Err(Error::CommitMaybeCommitted),
        CommitOutcome::RolledBack => {
            Err(Error::TransactionState("index-format upgrade rolled back"))
        }
    }
}

/// A UNIQUE conflict while upgrading means the old key format kept apart
/// values the current one treats as equal; say so and say what to do.
fn describe_upgrade_failure(snapshot: &SchemaSnapshot, index_id: IndexId, err: Error) -> Error {
    let Error::ConstraintViolation(detail) = err else {
        return err;
    };
    let index = snapshot
        .index_by_id(index_id)
        .map(|index| index.name.to_string())
        .unwrap_or_else(|| format!("#{}", index_id.0));
    Error::ConstraintViolation(format!(
        "{detail}: cannot rebuild UNIQUE index {index} for index format {INDEX_VERSION}, \
         which gives numerically equal INTEGER and REAL values (such as 1 and 1.0) one key; \
         the database was not changed; delete the duplicate rows with RedlineDB 4.x, then \
         open it again"
    ))
}

/// The indexes `target` names, in catalog order.
fn resolve_target(
    conn: &Connection,
    session: &SessionState,
    snapshot: &SchemaSnapshot,
    target: &ReindexTarget,
) -> Result<Vec<IndexId>> {
    let physical = |index: &&Arc<IndexDef>| index.meta_page_id.is_some();
    let (schema, name) = match target {
        ReindexTarget::All => {
            return Ok(snapshot
                .indexes
                .iter()
                .filter(physical)
                .map(|index| index.index_id)
                .collect());
        }
        ReindexTarget::Named { schema, name } => (schema.as_deref(), name.as_ref()),
    };
    // `None`: any table; `Some(true)`: TEMP tables only; `Some(false)`: the
    // rest.
    let temp_only = match schema {
        None => None,
        Some(schema) if schema.eq_ignore_ascii_case("main") => Some(false),
        Some(schema) if schema.eq_ignore_ascii_case("temp") => Some(true),
        Some(schema) if conn.attach_map().database(schema).is_some() => {
            return Err(Error::UnsupportedSql(format!(
                "REINDEX of an attached database is not supported: {schema}"
            )));
        }
        Some(schema) => return Err(Error::Parse(format!("unknown database {schema}"))),
    };
    if schema.is_none() && is_collation_name(snapshot, name) {
        return Ok(snapshot
            .indexes
            .iter()
            .filter(physical)
            .filter(|index| {
                index.keys.iter().any(|key| {
                    matches!(key.source, IndexKeySource::Column { .. })
                        && key
                            .collation
                            .as_deref()
                            .unwrap_or("BINARY")
                            .eq_ignore_ascii_case(name)
                })
            })
            .map(|index| index.index_id)
            .collect());
    }
    let in_scope = |table_name: &str| {
        let is_temp = session
            .temp_tables
            .iter()
            .any(|temp| temp.eq_ignore_ascii_case(table_name));
        temp_only.is_none_or(|want_temp| want_temp == is_temp)
    };
    let main = redlinedb_kernel::catalog::resolve_schema_id(snapshot, None)?;
    if let Some(table) = snapshot.lookup_table(main, name)
        && in_scope(&table.name)
    {
        return Ok(table
            .indexes
            .iter()
            .filter(|index| index.meta_page_id.is_some())
            .map(|index| index.index_id)
            .collect());
    }
    if let Some(index) = snapshot.lookup_index(main, name)
        && snapshot
            .table_by_id(index.table_id)
            .is_some_and(|table| in_scope(&table.name))
    {
        return Ok(if index.meta_page_id.is_some() {
            vec![index.index_id]
        } else {
            Vec::new()
        });
    }
    Err(Error::Parse(
        "unable to identify the object to be reindexed".to_owned(),
    ))
}

/// The built-in collations, and any collation an index key uses (which the
/// connection must have registered for the index to work at all).
fn is_collation_name(snapshot: &SchemaSnapshot, name: &str) -> bool {
    ["BINARY", "NOCASE", "RTRIM"]
        .iter()
        .any(|builtin| builtin.eq_ignore_ascii_case(name))
        || snapshot.indexes.iter().any(|index| {
            index.keys.iter().any(|key| {
                key.collation
                    .as_deref()
                    .is_some_and(|collation| collation.eq_ignore_ascii_case(name))
            })
        })
}
