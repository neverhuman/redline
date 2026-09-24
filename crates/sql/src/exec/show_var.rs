//! `SHOW` results. The body lives here so `exec/mod.rs` stays under the line cap.

use std::sync::Arc;

use crate::batch::QueryMemoryBroker;
use crate::connection::Connection;
use crate::error::Result;
use crate::statement::{
    ExecutionResult, RuntimeState, SelectRuntime, SelectRuntimeSource, SelectRuntimeTx,
};
use crate::value::SqlValue;

use super::with_session_reentrant;

pub(crate) fn show_done(conn: &Connection, name: &str) -> Result<ExecutionResult> {
    let value = if name.eq_ignore_ascii_case("transaction_isolation") {
        let iso = with_session_reentrant(conn, |session| Ok(session.transaction_isolation))?;
        SqlValue::Text(Arc::from(iso.as_pg_str()))
    } else if name.eq_ignore_ascii_case("search_path") {
        let path = with_session_reentrant(conn, |session| Ok(session.search_path.clone()))?;
        SqlValue::Text(Arc::from(path))
    } else if name.eq_ignore_ascii_case("wal_level") {
        SqlValue::Text(Arc::from("replica"))
    } else if name.eq_ignore_ascii_case("session_replication_role") {
        SqlValue::Text(Arc::from("origin"))
    } else {
        SqlValue::Text(Arc::from(""))
    };
    Ok(ExecutionResult {
        runtime: RuntimeState::Select(SelectRuntime {
            tx: SelectRuntimeTx::Empty,
            restore_tx: false,
            source: SelectRuntimeSource::StaticRows {
                rows: Arc::from(vec![vec![value]]),
                cursor: 0,
            },
            selection: None,
            projection: Vec::new(),
            limit: usize::MAX,
            offset: 0,
            seen: 0,
            yielded: 0,
            memory: QueryMemoryBroker::new(0, 0, None),
        }),
        affected_rows: 0,
    })
}
