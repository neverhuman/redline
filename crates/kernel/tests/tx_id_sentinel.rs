//! `TxId(u64::MAX)` is the index's non-transactional delete marker, not a
//! transaction id.
//!
//! `BtreeIndex::delete_mark` logs an index delete under that id. Recovery
//! moves the next transaction id past every id the WAL names, and the last
//! id has no successor, so treating the marker as a transaction made every
//! later open fail with "wal names a transaction id with no successor".
//! The marker must also never be handed out: an id whose successor is the
//! marker has no usable successor either.

use std::sync::Arc;
use std::time::Duration;

use redlinedb_kernel::Error;
use redlinedb_kernel::catalog::{
    ColumnSpec, CreateIndexSpec, CreateTableSpec, DbName, IndexColumnSpec, IndexOrigin,
    QualifiedName, SortDir, ValueRef, encode_index_key, encode_record,
};
use redlinedb_kernel::engine::{CommitDurability, Engine, EngineConfig};
use redlinedb_kernel::format::{DEFAULT_PAGE_SIZE, RelId, RowId, TxId};
use redlinedb_kernel::txn::Isolation;
use redlinedb_kernel::wal::{WalConfig, WalManager, WalPayload, WalRecordKind};
use tempfile::TempDir;

fn config() -> EngineConfig {
    EngineConfig {
        rel_id: RelId(1),
        wal: WalConfig {
            segment_bytes: 65_536,
            ..WalConfig::default()
        },
        commit_durability: CommitDurability::Strict,
        lock_shards: 32,
        busy_timeout: Duration::from_millis(250),
        heap_lanes: 4,
        page_size: DEFAULT_PAGE_SIZE,
        buffer_pool_pages: 256,
        data_file_name: "data.redline".to_owned(),
    }
}

fn column(name: &str, declared_type: &str) -> ColumnSpec {
    ColumnSpec {
        name: DbName::new(name),
        declared_type: Some(declared_type.to_owned()),
        constraints: vec![],
        collation: None,
        default_value: None,
        autoincrement: false,
        generated: None,
    }
}

fn create_table(engine: &Arc<Engine>) {
    let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
    engine
        .create_table(
            &mut tx,
            CreateTableSpec {
                schema: None,
                name: DbName::new("kv"),
                if_not_exists: false,
                columns: vec![column("k", "INTEGER"), column("v", "TEXT")],
                constraints: vec![],
                strict: false,
                without_rowid: false,
                normalized_sql: Some("CREATE TABLE kv(k INTEGER, v TEXT)".to_owned()),
            },
        )
        .expect("create table");
    engine.commit(tx).expect("commit");
}

/// Built over the rows already in `kv`; kernel inserts do not maintain it.
fn create_index(engine: &Arc<Engine>) {
    let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
    engine
        .create_index(
            &mut tx,
            CreateIndexSpec {
                schema: None,
                name: DbName::new("ix_kv_k"),
                if_not_exists: false,
                table: QualifiedName {
                    schema: DbName::new("main"),
                    name: DbName::new("kv"),
                },
                unique: false,
                columns: vec![IndexColumnSpec {
                    name: DbName::new("k"),
                    sort_dir: SortDir::Asc,
                    collation: None,
                    expr_sql: None,
                    expr_referenced_cols: Vec::new(),
                }],
                origin: IndexOrigin::User,
                normalized_sql: Some("CREATE INDEX ix_kv_k ON kv(k)".to_owned()),
                predicate_sql: None,
            },
        )
        .expect("create index");
    engine.commit(tx).expect("commit");
}

fn insert_row(engine: &Arc<Engine>, key: i64) -> RowId {
    let table = engine
        .schema_snapshot()
        .tables
        .iter()
        .find(|t| t.name.as_ref() == "kv")
        .cloned()
        .expect("kv table");
    let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
    let row_id = engine.reserve_row_id();
    let mut payload = Vec::new();
    encode_record(&[ValueRef::Integer(key), ValueRef::Text("v")], &mut payload)
        .expect("encode record");
    engine
        .insert_for_relation(&mut tx, table.relation_id, row_id, payload)
        .expect("insert");
    engine.commit(tx).expect("commit");
    row_id
}

fn index_key(value: i64) -> Vec<u8> {
    let mut buf = Vec::new();
    encode_index_key(&[ValueRef::Integer(value)], &[SortDir::Asc], &mut buf).bytes
}

#[test]
fn a_non_transactional_index_delete_does_not_stop_the_next_open() {
    let temp = TempDir::new().expect("temp dir");
    let engine = Engine::create(temp.path(), config()).expect("create");
    create_table(&engine);
    let row = insert_row(&engine, 42);
    insert_row(&engine, 7);
    create_index(&engine);
    let index_id = engine
        .schema_snapshot()
        .indexes
        .iter()
        .find(|i| i.name.as_ref() == "ix_kv_k")
        .expect("index")
        .index_id;
    let index = engine.index_handle(index_id).expect("index handle");
    let entry = index
        .iter_all_entries()
        .expect("dump index")
        .into_iter()
        .find(|entry| entry.row.row_id == row)
        .expect("entry for k=42");
    index
        .delete_mark(&index_key(42), entry.row)
        .expect("non-transactional delete");
    drop(index);
    drop(engine);

    let reopened = Engine::open(temp.path(), config())
        .expect("a WAL holding a non-transactional index delete must open");
    let tx = reopened.begin(Isolation::Snapshot).expect("begin");
    assert!(
        tx.id().0 < u64::MAX - 1,
        "the next transaction id {} ran into the delete marker",
        tx.id().0
    );
    reopened.rollback(tx).expect("rollback");
    drop(reopened);
    // And again, now that recovery has run over that record once.
    Engine::open(temp.path(), config()).expect("second reopen");
}

/// A record naming `u64::MAX - 1` would make the delete marker the next
/// transaction id, so it has no usable successor either.
#[test]
fn a_wal_id_whose_successor_is_the_delete_marker_fails_open() {
    let temp = TempDir::new().expect("temp dir");
    let engine = Engine::create(temp.path(), config()).expect("create");
    let mut tx = engine.begin(Isolation::Snapshot).expect("begin");
    engine.insert(&mut tx, b"base".to_vec()).expect("insert");
    engine.commit(tx).expect("commit");
    drop(engine);

    let mut wal = WalManager::open(temp.path().join("wal"), config().wal).expect("wal");
    let tx_id = TxId(u64::MAX - 1);
    let payload = WalPayload::HeapInsert {
        tx_id,
        rel_id: RelId(1),
        row_id: RowId(99),
        payload: b"never committed".to_vec(),
    }
    .encode()
    .expect("encode");
    wal.append(WalRecordKind::PageDelta, tx_id, payload)
        .expect("append");
    wal.flush().expect("flush");
    drop(wal);

    let err = Engine::open(temp.path(), config()).expect_err("open must refuse");
    assert!(matches!(err, Error::CorruptWal(_)), "unexpected {err:?}");
}
