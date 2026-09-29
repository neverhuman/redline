//! Reading collations back: from a format-7 catalog, and from the COLLATE
//! clauses of CREATE TABLE text.

use super::*;

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
