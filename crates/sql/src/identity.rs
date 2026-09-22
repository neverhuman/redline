//! Session counters for `GENERATED { ALWAYS | BY DEFAULT } AS IDENTITY`.
//!
//! The counter is not `max(rowid) + 1`. An explicit insert into a
//! `BY DEFAULT` column leaves the next generated value at the sequence
//! start. `ALWAYS` rejects an explicit target list entry.

use redlinedb_kernel::catalog::{AlterTableOperationSpec, AlterTableSpec, TableDef};

use crate::connection::Connection;
use crate::error::Result;
use crate::exec::with_session_reentrant;
use crate::session::{IdentityColumn, SessionState};
use crate::value::SqlValue;

#[derive(Debug, Clone)]
pub(crate) struct IdentitySpec {
    pub column: String,
    pub always: bool,
    pub start: i64,
    pub increment: i64,
}

pub(crate) fn remember(conn: &Connection, table: &str, specs: &[IdentitySpec]) -> Result<()> {
    if specs.is_empty() {
        return Ok(());
    }
    with_session_reentrant(conn, |session| {
        for spec in specs {
            let key = identity_key(table, &spec.column);
            session.pg_identities.entry(key).or_insert(IdentityColumn {
                always: spec.always,
                start: spec.start,
                increment: spec.increment,
                last_value: None,
            });
        }
        Ok(())
    })
}

pub(crate) fn forget_table(session: &mut SessionState, table: &str) {
    let prefix = format!("{}\u{1}", table.to_ascii_lowercase());
    session
        .pg_identities
        .retain(|key, _| !key.starts_with(&prefix));
}

pub(crate) fn is_always(conn: &Connection, table: &str, column: &str) -> bool {
    with_session_reentrant(conn, |session| {
        Ok(session
            .pg_identities
            .get(&identity_key(table, column))
            .is_some_and(|column| column.always))
    })
    .unwrap_or(false)
}

/// Fill identity columns that this INSERT did not name.
pub(crate) fn fill_omitted(
    session: &mut SessionState,
    table: &TableDef,
    provided: &[usize],
    values: &mut [SqlValue],
) {
    for (idx, column) in table.columns.iter().enumerate() {
        if provided.iter().any(|ordinal| *ordinal == idx) {
            continue;
        }
        let key = identity_key(table.folded.as_ref(), column.folded.as_ref());
        let Some(entry) = session.pg_identities.get_mut(&key) else {
            continue;
        };
        let next = match entry.last_value {
            Some(value) => value.saturating_add(entry.increment),
            None => entry.start,
        };
        entry.last_value = Some(next);
        if let Some(slot) = values.get_mut(idx) {
            *slot = SqlValue::Integer(next);
        }
    }
}

pub(crate) fn after_alter(conn: &Connection, sql: &str, spec: &AlterTableSpec) -> Result<()> {
    let table = spec.name.name.folded();
    match &spec.operation {
        AlterTableOperationSpec::AddColumnIdentity {
            column_name,
            always,
            start,
            increment,
        } => remember(
            conn,
            table,
            &[IdentitySpec {
                column: column_name.original().to_owned(),
                always: *always,
                start: *start,
                increment: *increment,
            }],
        ),
        AlterTableOperationSpec::DropColumnNotNull { column_name }
            if contains_drop_identity(sql) =>
        {
            let key = identity_key(table, column_name.folded());
            with_session_reentrant(conn, |session| {
                session.pg_identities.remove(&key);
                Ok(())
            })
        }
        _ => Ok(()),
    }
}

fn contains_drop_identity(sql: &str) -> bool {
    let lower = sql.to_ascii_lowercase();
    lower.contains("drop identity")
}

fn identity_key(table: &str, column: &str) -> String {
    format!(
        "{}\u{1}{}",
        table.to_ascii_lowercase(),
        column.to_ascii_lowercase()
    )
}
