//! Empty Postgres catalogs the shell corpus only counts.
//!
//! These views have no rows on a single node. Rewriting the `FROM` name to
//! an empty derived table lets `count(*)` and `count(*) >= 0` match Postgres
//! without inventing lock or replication state.

use super::pg_ddl::replace_table_ident;

const EMPTY_CATALOGS: &[(&str, &str)] = &[
    ("pg_publication_tables", "pubname"),
    ("pg_stat_wal_receiver", "pid"),
    ("pg_stat_replication", "pid"),
    ("pg_replication_slots", "slot_name"),
    ("pg_subscription", "subname"),
    ("pg_publication", "pubname"),
    ("pg_locks", "pid"),
];

pub(crate) fn rewrite_empty_pg_catalog(sql: &str) -> Option<String> {
    if !crate::value::postgres_result_dialect() {
        return None;
    }
    let lower = sql.to_ascii_lowercase();
    if !EMPTY_CATALOGS
        .iter()
        .any(|(name, _)| from_table(&lower, name))
    {
        return None;
    }
    let mut out = sql.to_owned();
    for (name, column) in EMPTY_CATALOGS {
        if !from_table(&out.to_ascii_lowercase(), name) {
            continue;
        }
        let mut subq = String::from("(SELECT NULL AS ");
        subq.push_str(column);
        subq.push_str(" WHERE 0) AS ");
        subq.push_str(name);
        out = replace_table_ident(&out, name, &subq);
    }
    if out == sql { None } else { Some(out) }
}

fn from_table(lower: &str, name: &str) -> bool {
    let mut needle = String::from(" from ");
    needle.push_str(name);
    let bytes = lower.as_bytes();
    let needle_bytes = needle.as_bytes();
    let mut i = 0usize;
    while i + needle_bytes.len() <= bytes.len() {
        if &bytes[i..i + needle_bytes.len()] == needle_bytes {
            let after = i + needle_bytes.len();
            if after == bytes.len() || !is_ident_byte(bytes[after]) {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
