#![no_main]

use std::sync::{Arc, OnceLock};

use libfuzzer_sys::fuzz_target;
use redlinedb_sql::{
    first_statement_complete, input_complete, is_blank_sql, split_first_statement,
    split_statements, Database, DbOptions, Dialect,
};

#[path = "../src/oracle.rs"]
mod oracle;

fn connection() -> Arc<redlinedb_sql::Connection> {
    static CELL: OnceLock<Arc<redlinedb_sql::Connection>> = OnceLock::new();
    CELL.get_or_init(|| {
        let mut opts = DbOptions::default();
        opts.dialect = Some(Dialect::Sqlite);
        if let Some(dir) = std::env::var_os("REDLINE_FUZZ_TMP") {
            opts.temp_dir = Some(dir.into());
        }
        let db = Database::create_in_memory(opts).expect("in-memory database");
        db.connect()
    })
    .clone()
}

fuzz_target!(|data: &[u8]| {
    if data.len() > 8 * 1024 {
        return;
    }
    let sql = String::from_utf8_lossy(data);
    let _ = is_blank_sql(&sql);
    let _ = input_complete(&sql);
    let _ = first_statement_complete(&sql);
    let (head, tail) = split_first_statement(&sql);
    let _ = (head, tail);
    let _ = split_statements(&sql);
    if oracle::numbered_parameter_exceeds_sqlite_cap(&sql) {
        return;
    }
    if let Err(err) = connection().prepare(sql.as_ref()) {
        if oracle::parser_panic(&err) {
            panic!("{err}");
        }
    }
});
