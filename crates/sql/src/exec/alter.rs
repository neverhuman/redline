use super::*;
use redlinedb_kernel::catalog::{AlterTableOperationSpec, resolve_schema_id};

pub(super) fn rewrite_drop_column_rows(
    conn: &Connection,
    tx: &mut Txn,
    spec: &redlinedb_kernel::catalog::AlterTableSpec,
) -> Result<()> {
    let AlterTableOperationSpec::DropColumn {
        column_name,
        if_exists: _,
    } = &spec.operation
    else {
        return Ok(());
    };

    let snapshot = conn.engine().schema_snapshot_for_tx(tx);
    let schema_id = resolve_schema_id(&snapshot, Some(&spec.name.schema))?;
    let Some(table) = snapshot.lookup_table(schema_id, spec.name.name.folded()) else {
        return Ok(());
    };
    let Some(drop_ordinal) = table
        .columns
        .iter()
        .position(|column| column.folded.as_ref() == column_name.folded())
    else {
        return Ok(());
    };

    let rows = collect_table_rows(conn.engine(), tx, &table)?;
    if rows.is_empty() {
        return Ok(());
    }

    let mut rewrites = Vec::with_capacity(rows.len());
    for row in rows {
        let rowid = row.rowid;
        let mut values = row.values;
        if drop_ordinal as usize >= values.len() {
            return Err(Error::UnsupportedSql(
                "ALTER TABLE DROP COLUMN rewrite hit short row payload".to_owned(),
            ));
        }
        values.remove(drop_ordinal);
        let payload = encode_sql_row(table.table_id.0, &values)?;
        rewrites.push((rowid, payload));
    }

    for (rowid, payload) in rewrites {
        conn.engine()
            .update_for_relation(tx, table.relation_id, rowid, payload)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tempfile::tempdir;

    use crate::connection::{Database, DbOptions};
    use crate::exec::tail::take_table_row_loads;
    use crate::statement::Step;

    #[test]
    fn drop_column_loads_each_live_row_once() {
        let dir = tempdir().unwrap();
        let db = Database::create(
            dir.path().join("drop-col.db"),
            DbOptions {
                busy_timeout: Duration::from_secs(5),
                ..DbOptions::default()
            },
        )
        .unwrap();
        let conn = db.connect();
        conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, extra INTEGER, v INTEGER)")
            .unwrap();
        let mut insert = conn
            .prepare("INSERT INTO t(id, extra, v) VALUES (?1, ?2, ?3)")
            .unwrap();
        for i in 0..20 {
            insert.bind_i64(1, i).unwrap();
            insert.bind_i64(2, i).unwrap();
            insert.bind_i64(3, i + 10).unwrap();
            assert_eq!(insert.step().unwrap(), Step::Done);
            insert.reset().unwrap();
        }
        let _ = take_table_row_loads();
        conn.execute("ALTER TABLE t DROP COLUMN extra").unwrap();
        assert_eq!(take_table_row_loads(), 20);
        let mut stmt = conn.prepare("SELECT v FROM t WHERE id = 4").unwrap();
        assert_eq!(stmt.step().unwrap(), Step::Row);
        assert_eq!(
            stmt.column_value(0).unwrap().clone(),
            crate::value::SqlValue::Integer(14)
        );
    }
}
