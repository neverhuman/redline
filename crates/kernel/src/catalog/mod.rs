mod affinity;
mod bootstrap;
mod codec;
mod ddl;
mod expr;
mod ids;
mod key;
mod key_epoch;
#[cfg(test)]
mod key_tests;
mod manager;
mod names;
mod numeric;
mod ops;
mod record;
mod schema;
mod stats;
mod store;
mod system;
mod triggers;
mod value;
mod views;

pub use affinity::{Affinity, CoerceError, apply_affinity, derive_affinity};
pub use bootstrap::bootstrap_schema;
pub use ddl::{
    AlterTableOperationSpec, AlterTableSpec, ColumnConstraintSpec, ColumnSpec, ConflictAction,
    CreateIndexSpec, CreateTableSpec, CreateTriggerSpec, CreateViewSpec, DropIndexSpec,
    DropTableSpec, DropTriggerSpec, DropViewSpec, FkAction, IndexColumnSpec, IndexOrigin,
    TableConstraintSpec, TriggerEventKind, TriggerTimeKind,
};
pub use expr::{
    CompiledExpr, EvalScratch, ExprAst, ExprError, ExprOp, RowValueSource, compile_expr, eval_expr,
};
pub use ids::{ColumnId, ConstraintId, IndexId, ObjectId, RelationId, SchemaId, TableId};
pub use key::{
    DecodedNumericKey, EncodedIndexKey, IndexKeyDef, IndexKeySource, NullOrder, SortDir,
    compare_index_keys, decode_numeric_key_part, encode_index_key, is_numeric_key_tag,
    numeric_key_part_len,
};
pub(crate) use key_epoch::v4_index_format_active;
#[doc(hidden)]
pub use key_epoch::with_v4_index_format_for_tests;
pub use manager::CatalogManager;
pub use names::{DbName, QualifiedName};
pub use numeric::{int_real_cmp, sqlite_numeric_prefix, sqlite_text_is_true};
pub use ops::{
    apply_alter_table, apply_create_index, apply_create_table, apply_drop_index, apply_drop_table,
    apply_rename_index, apply_set_index_meta_page_id, legacy_alter_table_active_for_tests,
    lookup_index, lookup_table, resolve_schema_id, set_legacy_alter_table,
};
pub use record::{RecordRef, RecordScratch, encode_record};
pub use schema::{
    CatalogError, CatalogMeta, CheckDef, ClassKind, ColumnDef, ConstraintDef, ConstraintKind,
    ForeignKeyDef, GeneratedColumnKind, GeneratedColumnSpec, IndexDef, NamespaceDef, SchemaEpoch,
    SchemaSnapshot, SqliteSchemaRow, TABLE_FLAG_STRICT, TABLE_FLAG_WITHOUT_ROWID, TableDef,
    TriggerDef, ViewDef,
};
pub use stats::{
    ColumnStats, HistogramBucket, IndexStats, MostCommonValue, StatsEpoch, StatsSnapshot,
    StatsStore, TableStats,
};
pub(crate) use store::CatalogSyncPolicy;
pub use store::{CatalogStore, decode_snapshot, encode_snapshot};
#[cfg(test)]
pub(crate) use store::{catalog_dir_syncs, catalog_metadata_syncs};
pub use system::*;
pub use triggers::{apply_create_trigger, apply_drop_trigger, triggers_for};
pub use value::{OwnedValue, StorageClass, ValueRef};
pub use views::{apply_create_view, apply_drop_view, lookup_view};
