//! Declared column collations and index key collations in the catalog
//! (workplan Q5-10).
//!
//! A new index key inherits its column's declared NOCASE or RTRIM unless it
//! names a collation; format 8 stores both collations; a format-7 catalog
//! reads the column collation back from its table SQL but keeps an index
//! key at the collation its CREATE INDEX text names, which is how its
//! B-tree was built; and `index_keys_needing_inherited_collation` names
//! such an index for the rebuild an upgrade must run.

use redlinedb_kernel::catalog::collation::{
    column_collations_from_create_table, index_keys_needing_inherited_collation,
};
use redlinedb_kernel::catalog::{
    AlterTableOperationSpec, AlterTableSpec, ColumnConstraintSpec, ColumnSpec, ConflictAction,
    CreateIndexSpec, CreateTableSpec, DbName, IndexColumnSpec, IndexOrigin, QualifiedName,
    SchemaSnapshot, SortDir, TableConstraintSpec, apply_alter_table, apply_create_index,
    apply_create_table, bootstrap_schema, decode_snapshot, encode_snapshot,
};
use redlinedb_kernel::format::RelId;

fn column(
    name: &str,
    collation: Option<&str>,
    constraints: Vec<ColumnConstraintSpec>,
) -> ColumnSpec {
    ColumnSpec {
        name: DbName::new(name),
        declared_type: Some("TEXT".to_owned()),
        constraints,
        collation: collation.map(str::to_owned),
        default_value: None,
        autoincrement: false,
        generated: None,
    }
}

fn table_name(name: &str) -> QualifiedName {
    QualifiedName {
        schema: DbName::new("main"),
        name: DbName::new(name),
    }
}

/// `t(a TEXT COLLATE NOCASE UNIQUE, b TEXT COLLATE RTRIM, c TEXT,
/// UNIQUE(b))`.
fn snapshot_with_table() -> SchemaSnapshot {
    let snapshot = (*bootstrap_schema(RelId(100))).clone();
    apply_create_table(
        snapshot,
        CreateTableSpec {
            schema: None,
            name: DbName::new("t"),
            if_not_exists: false,
            columns: vec![
                column(
                    "a",
                    Some("nocase"),
                    vec![ColumnConstraintSpec::Unique {
                        conflict: ConflictAction::Abort,
                    }],
                ),
                column("b", Some("\"RTRIM\""), Vec::new()),
                column("c", None, Vec::new()),
            ],
            constraints: vec![TableConstraintSpec::Unique {
                name: None,
                columns: vec![DbName::new("b")],
                conflict: ConflictAction::Abort,
            }],
            strict: false,
            without_rowid: false,
            normalized_sql: Some(
                "CREATE TABLE t(a TEXT COLLATE NOCASE UNIQUE, b TEXT COLLATE \"RTRIM\", c TEXT, UNIQUE(b))"
                    .to_owned(),
            ),
        },
    )
    .expect("create table")
}

fn index(
    snapshot: SchemaSnapshot,
    name: &str,
    column: &str,
    collation: Option<&str>,
) -> SchemaSnapshot {
    apply_create_index(
        snapshot,
        CreateIndexSpec {
            schema: None,
            name: DbName::new(name),
            if_not_exists: false,
            table: table_name("t"),
            unique: false,
            columns: vec![IndexColumnSpec {
                name: DbName::new(column),
                sort_dir: SortDir::Asc,
                collation: collation.map(str::to_owned),
                expr_sql: None,
                expr_referenced_cols: Vec::new(),
            }],
            origin: IndexOrigin::User,
            normalized_sql: Some(format!("CREATE INDEX {name} ON t({column})")),
            predicate_sql: None,
        },
    )
    .expect("create index")
}

fn key_collations(snapshot: &SchemaSnapshot) -> Vec<(String, Option<String>)> {
    let table = snapshot
        .tables
        .iter()
        .find(|t| t.name.as_ref() == "t")
        .expect("table t");
    let mut out: Vec<(String, Option<String>)> = table
        .indexes
        .iter()
        .map(|index| {
            (
                index.name.to_string(),
                index.keys[0].collation.as_deref().map(str::to_owned),
            )
        })
        .collect();
    out.sort();
    out
}

fn column_collations(snapshot: &SchemaSnapshot) -> Vec<Option<String>> {
    snapshot
        .tables
        .iter()
        .find(|t| t.name.as_ref() == "t")
        .expect("table t")
        .columns
        .iter()
        .map(|column| column.collation.as_deref().map(str::to_owned))
        .collect()
}

fn s(value: &str) -> Option<String> {
    Some(value.to_owned())
}

#[test]
fn new_index_keys_inherit_the_declared_collation_unless_they_name_one() {
    let snapshot = snapshot_with_table();
    let snapshot = index(snapshot, "ia", "a", None);
    let snapshot = index(snapshot, "ia_bin", "a", Some("binary"));
    let snapshot = index(snapshot, "ic_nocase", "c", Some("nocase"));
    assert_eq!(
        column_collations(&snapshot),
        [s("NOCASE"), s("RTRIM"), None]
    );
    let keys = key_collations(&snapshot);
    let get = |name: &str| {
        keys.iter()
            .find(|(n, _)| n.as_str() == name)
            .unwrap_or_else(|| panic!("{name} in {keys:?}"))
            .1
            .clone()
    };
    assert_eq!(get("ia"), s("NOCASE"));
    assert_eq!(get("ia_bin"), s("BINARY"));
    assert_eq!(get("ic_nocase"), s("NOCASE"));
    // The UNIQUE constraints' own indexes inherit too.
    let auto: Vec<Option<String>> = keys
        .iter()
        .filter(|(name, _)| name.starts_with("sqlite_autoindex"))
        .map(|(_, collation)| collation.clone())
        .collect();
    assert_eq!(auto.len(), 2, "{keys:?}");
    assert!(
        auto.contains(&s("NOCASE")) && auto.contains(&s("RTRIM")),
        "{keys:?}"
    );
    let table = snapshot
        .tables
        .iter()
        .find(|t| t.name.as_ref() == "t")
        .unwrap();
    for index in &table.indexes {
        assert_eq!(
            index_keys_needing_inherited_collation(table, index),
            None,
            "{} was built with its inherited collation",
            index.name
        );
    }
}

#[test]
fn format_8_stores_column_and_key_collations() {
    let snapshot = index(snapshot_with_table(), "ia", "a", None);
    let bytes = encode_snapshot(&snapshot).expect("encode");
    assert_eq!(u64::from_le_bytes(bytes[..8].try_into().unwrap()), 8);
    let decoded = decode_snapshot(&bytes).expect("decode");
    assert_eq!(column_collations(&decoded), column_collations(&snapshot));
    assert_eq!(key_collations(&decoded), key_collations(&snapshot));
}

#[test]
fn a_collation_survives_alter_table_add_column() {
    let snapshot = snapshot_with_table();
    let snapshot = apply_alter_table(
        snapshot,
        AlterTableSpec {
            name: table_name("t"),
            if_exists: false,
            operation: AlterTableOperationSpec::AddColumn {
                column: column("d", Some("NOCASE"), Vec::new()),
                if_not_exists: false,
                table_constraints: Vec::new(),
            },
        },
    )
    .expect("add column");
    // ADD COLUMN drops the table SQL, so only format 8 keeps the collations.
    let decoded = decode_snapshot(&encode_snapshot(&snapshot).expect("encode")).expect("decode");
    assert_eq!(
        column_collations(&decoded),
        [s("NOCASE"), s("RTRIM"), None, s("NOCASE")]
    );
    let sql = decoded
        .sqlite_schema_rows()
        .into_iter()
        .find(|row| row.name.as_ref() == "t")
        .expect("t in sqlite_schema")
        .sql;
    assert!(sql.contains("d TEXT COLLATE NOCASE"), "{sql}");
}

/// A format-7 catalog: the same table, written before collations were
/// stored. Its index on the NOCASE column was built with BINARY keys.
fn format_7_bytes() -> Vec<u8> {
    let mut snapshot = index(snapshot_with_table(), "ia", "a", None);
    snapshot.meta.format_version = 7;
    for table in &mut snapshot.tables {
        let table = std::sync::Arc::make_mut(table);
        for column in &mut table.columns {
            column.collation = None;
        }
        for index in &mut table.indexes {
            for key in &mut index.keys {
                key.collation = None;
            }
        }
    }
    let bytes = encode_snapshot(&snapshot).expect("encode");
    assert_eq!(u64::from_le_bytes(bytes[..8].try_into().unwrap()), 7);
    bytes
}

#[test]
fn a_format_7_catalog_reads_column_collations_from_sql_and_keeps_key_collations() {
    let decoded = decode_snapshot(&format_7_bytes()).expect("decode");
    assert_eq!(column_collations(&decoded), [s("NOCASE"), s("RTRIM"), None]);
    // The keys stay as built: reading them as NOCASE would probe BINARY
    // keys with case-folded values.
    assert!(
        key_collations(&decoded).iter().all(|(_, c)| c.is_none()),
        "{:?}",
        key_collations(&decoded)
    );
    // The upgrade hook names every index whose keys should inherit.
    let table = decoded
        .tables
        .iter()
        .find(|t| t.name.as_ref() == "t")
        .unwrap();
    let mut needing: Vec<(String, Vec<Option<String>>)> = table
        .indexes
        .iter()
        .filter_map(|index| {
            index_keys_needing_inherited_collation(table, index).map(|keys| {
                (
                    index.name.to_string(),
                    keys.into_iter()
                        .map(|k| k.map(|k| k.into_string()))
                        .collect(),
                )
            })
        })
        .collect();
    needing.sort();
    assert_eq!(needing.len(), 3, "{needing:?}");
    assert!(
        needing
            .iter()
            .any(|(name, keys)| name == "ia" && keys == &[s("NOCASE")])
    );
    assert!(needing.iter().any(|(_, keys)| keys == &[s("RTRIM")]));
}

fn declared(sql: &str) -> Vec<(String, String)> {
    column_collations_from_create_table(sql)
        .into_iter()
        .map(|(name, collation)| (name.into_string(), collation.into_string()))
        .collect()
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect()
}

#[test]
fn reads_each_column_collation_from_create_table() {
    assert_eq!(
        declared(
            "CREATE TABLE \"my t\" ( \"my col\" text   collate nocase unique, b INT CHECK (b > 0), c TEXT COLLATE \"RTRIM\", d VARCHAR(10) COLLATE binary, UNIQUE(c) )"
        ),
        pairs(&[("my col", "NOCASE"), ("c", "RTRIM")])
    );
    assert_eq!(
        declared(
            "create temporary table if not exists main.t([a,b] TEXT COLLATE NOCASE, `x` COLLATE rtrim, y TEXT DEFAULT 'COLLATE NOCASE', z TEXT CHECK (z <> 'a' COLLATE NOCASE), CONSTRAINT k PRIMARY KEY (y))"
        ),
        pairs(&[("a,b", "NOCASE"), ("x", "RTRIM")])
    );
    assert_eq!(
        declared("CREATE TABLE t(x TEXT COLLATE NOCASE COLLATE RTRIM) -- COLLATE NOCASE"),
        pairs(&[("x", "RTRIM")])
    );
    assert_eq!(
        declared("CREATE TABLE t(x TEXT /* COLLATE NOCASE */, y COLLATE my_coll)"),
        pairs(&[("y", "my_coll")])
    );
    assert!(declared("CREATE TABLE t AS SELECT x COLLATE NOCASE FROM u").is_empty());
    assert!(declared("CREATE INDEX i ON t(x COLLATE NOCASE)").is_empty());
    assert!(declared("CREATE VIRTUAL TABLE v USING fts5(x)").is_empty());
}
