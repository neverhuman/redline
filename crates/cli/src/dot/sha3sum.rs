//! `.sha3sum`: sqlite3's SHA3 hash of database content.
//!
//! sqlite3 hashes, per table (tables in name order, folded to lower case),
//! the text `S<len>:SELECT * FROM "<table>" NOT INDEXED;` and then each row
//! as `R` followed by each value: `N` for NULL, `I` and the integer's eight
//! big-endian bytes, `F` and the IEEE bits of a REAL the same way,
//! `T<len>:` and the UTF-8 of a TEXT, `B<len>:` and the bytes of a BLOB
//! (`sha3_query` in the shell's shathree extension). Rows are read in
//! table order: rowid order, or primary-key order for a WITHOUT ROWID
//! table. With a LIKE pattern it hashes each matching table on its own and
//! labels each hash; `--schema` also hashes the schema rows as RedlineDB
//! stores them. The digest is SHA3-224 unless an option picks another size.

use redlinedb::{Step, ValueRef};

use super::{CliState, DotOutcome};
use crate::render::{Cell, render_query};

const USAGE: &str = ".sha3sum ...             Compute a SHA3 hash of database content
    Options:
      --schema              Also hash the sqlite_schema table
      --sha3-224            Use the sha3-224 algorithm
      --sha3-256            Use the sha3-256 algorithm (default)
      --sha3-384            Use the sha3-384 algorithm
      --sha3-512            Use the sha3-512 algorithm
    Any other argument is a LIKE pattern for tables to hash";

const SCHEMA_QUERY: &str = "SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY name;";

pub fn sha3sum(state: &mut CliState, args: &[&str]) -> Result<DotOutcome, String> {
    let mut bits = 224;
    let mut with_schema = false;
    let mut pattern = None;
    for arg in args {
        match arg.trim_start_matches('-') {
            _ if !arg.starts_with('-') => pattern = Some(*arg),
            "schema" => with_schema = true,
            "sha3-224" => bits = 224,
            "sha3-256" => bits = 256,
            "sha3-384" => bits = 384,
            "sha3-512" => bits = 512,
            _ => {
                // sqlite3 prints the help to its output and the error to stderr.
                state
                    .output
                    .write_line(USAGE)
                    .map_err(|err| err.to_string())?;
                return Err(format!("Unknown option \"{arg}\" on \"sha3sum\""));
            }
        }
    }
    let mut conn = state.db.connect().map_err(|err| err.to_string())?;
    let tables = tables_to_hash(&mut conn, pattern, with_schema)?;
    if tables.is_empty() {
        return Err(".sha3sum failed.".to_owned());
    }
    let mut labelled = Vec::with_capacity(tables.len());
    let mut all = Vec::new();
    for table in &tables {
        let content = table_content(&mut conn, table)?;
        if pattern.is_some() {
            labelled.push((hex(&sha3(&content, bits)), table.clone()));
        } else {
            all.extend_from_slice(&content);
        }
    }
    // The result prints through the current output mode, as sqlite3's
    // query does.
    let (columns, rows) = if pattern.is_some() {
        let rows = labelled
            .into_iter()
            .map(|(hash, label)| vec![Cell::Text(hash), Cell::Text(label)])
            .collect();
        (vec!["hash".to_owned(), "label".to_owned()], rows)
    } else {
        let hash = hex(&sha3(&all, bits));
        (vec!["hash".to_owned()], vec![vec![Cell::Text(hash)]])
    };
    drop(conn);
    let widths: Vec<usize> = state
        .widths
        .iter()
        .map(|w| w.unsigned_abs() as usize)
        .collect();
    render_query(
        &mut state.output,
        state.mode,
        &state.separator,
        state.show_header,
        &state.null_value,
        &state.insert_table_name,
        &widths,
        &columns,
        &rows,
    )?;
    Ok(DotOutcome::Ok)
}

/// The tables sqlite3 hashes, folded to lower case and in name order.
fn tables_to_hash(
    conn: &mut redlinedb::Connection,
    pattern: Option<&str>,
    with_schema: bool,
) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT lower(name) FROM sqlite_master WHERE type = 'table' \
             AND name NOT LIKE 'sqlite_%' AND sql NOT LIKE 'CREATE VIRTUAL%' \
             AND (?1 IS NULL OR lower(name) LIKE ?1) \
             UNION ALL SELECT 'sqlite_schema' WHERE ?2 AND (?1 IS NULL OR 'sqlite_schema' LIKE ?1)",
        )
        .map_err(|err| err.to_string())?;
    match pattern {
        Some(pattern) => stmt.bind_text(1, pattern),
        None => stmt.bind_null(1),
    }
    .map_err(|err| err.to_string())?;
    stmt.bind_i64(2, i64::from(with_schema))
        .map_err(|err| err.to_string())?;
    let mut tables = Vec::new();
    while let Step::Row(row) = stmt.step().map_err(|err| err.to_string())? {
        tables.push(row.get::<String>(0).map_err(|err| err.to_string())?);
    }
    tables.sort();
    Ok(tables)
}

/// What `sha3_query` feeds the hash for one table: its query text, then its
/// rows.
fn table_content(conn: &mut redlinedb::Connection, table: &str) -> Result<Vec<u8>, String> {
    let (hashed_sql, run_sql) = if table == "sqlite_schema" {
        (SCHEMA_QUERY.to_owned(), SCHEMA_QUERY.to_owned())
    } else {
        let name = quote_ident(table);
        let order = row_order(conn, table)?;
        (
            format!("SELECT * FROM {name} NOT INDEXED;"), // jankurai:allow HLT-023-INPUT-BOUNDARY-GAP reason=table-identifier-escaped-via-quote-ident-not-a-bindable-data-value expires=2027-06-01
            format!("SELECT * FROM {name} ORDER BY {order}"), // jankurai:allow HLT-023-INPUT-BOUNDARY-GAP reason=table-identifier-escaped-via-quote-ident-not-a-bindable-data-value expires=2027-06-01
        )
    };
    let mut out = format!("S{}:{hashed_sql}", hashed_sql.len()).into_bytes();
    let mut stmt = conn.prepare(&run_sql).map_err(|err| err.to_string())?;
    let columns = stmt.column_count();
    while let Step::Row(row) = stmt.step().map_err(|err| err.to_string())? {
        out.push(b'R');
        for index in 0..columns {
            match row.get_ref(index).map_err(|err| err.to_string())? {
                ValueRef::Null => out.push(b'N'),
                ValueRef::Integer(value) => {
                    out.push(b'I');
                    out.extend_from_slice(&value.to_be_bytes());
                }
                ValueRef::Real(value) => {
                    out.push(b'F');
                    out.extend_from_slice(&value.to_bits().to_be_bytes());
                }
                ValueRef::Text(value) => {
                    out.extend_from_slice(format!("T{}:", value.len()).as_bytes());
                    out.extend_from_slice(value.as_bytes());
                }
                ValueRef::Blob(value) => {
                    out.extend_from_slice(format!("B{}:", value.len()).as_bytes());
                    out.extend_from_slice(value);
                }
            }
        }
    }
    Ok(out)
}

/// sqlite3 scans the table's own b-tree: rowid order, or primary-key order
/// for a WITHOUT ROWID table.
fn row_order(conn: &mut redlinedb::Connection, table: &str) -> Result<String, String> {
    let mut stmt = conn
        .prepare("SELECT sql FROM sqlite_master WHERE type = 'table' AND lower(name) = ?1")
        .map_err(|err| err.to_string())?;
    stmt.bind_text(1, table).map_err(|err| err.to_string())?;
    let sql = match stmt.step().map_err(|err| err.to_string())? {
        Step::Row(row) => row.get::<String>(0).map_err(|err| err.to_string())?,
        Step::Done => String::new(),
    };
    drop(stmt);
    let compact: String = sql
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if !compact.contains("without rowid") {
        return Ok("rowid".to_owned());
    }
    let mut info = conn
        .prepare(&format!("PRAGMA table_info({})", quote_ident(table))) // jankurai:allow HLT-023-INPUT-BOUNDARY-GAP reason=table-identifier-escaped-via-quote-ident-not-a-bindable-data-value expires=2027-06-01
        .map_err(|err| err.to_string())?;
    let mut keys: Vec<(i64, String)> = Vec::new();
    while let Step::Row(row) = info.step().map_err(|err| err.to_string())? {
        let position: i64 = row.get(5).map_err(|err| err.to_string())?;
        if position > 0 {
            let name: String = row.get(1).map_err(|err| err.to_string())?;
            keys.push((position, quote_ident(&name)));
        }
    }
    keys.sort();
    Ok(keys
        .into_iter()
        .map(|(_, name)| name)
        .collect::<Vec<_>>()
        .join(", "))
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// SHA3-`bits` (FIPS 202) of `input`.
fn sha3(input: &[u8], bits: usize) -> Vec<u8> {
    let rate = 200 - 2 * (bits / 8);
    let mut state = [0u64; 25];
    let mut padded = input.to_vec();
    padded.push(0x06);
    while padded.len() % rate != 0 {
        padded.push(0);
    }
    if let Some(last) = padded.last_mut() {
        *last |= 0x80;
    }
    for block in padded.chunks(rate) {
        for (lane, word) in block.chunks(8).enumerate() {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(word);
            state[lane] ^= u64::from_le_bytes(bytes);
        }
        keccak_f1600(&mut state);
    }
    state
        .iter()
        .flat_map(|lane| lane.to_le_bytes())
        .take(bits / 8)
        .collect()
}

const ROUND_CONSTANTS: [u64; 24] = [
    0x0000000000000001,
    0x0000000000008082,
    0x800000000000808a,
    0x8000000080008000,
    0x000000000000808b,
    0x0000000080000001,
    0x8000000080008081,
    0x8000000000008009,
    0x000000000000008a,
    0x0000000000000088,
    0x0000000080008009,
    0x000000008000000a,
    0x000000008000808b,
    0x800000000000008b,
    0x8000000000008089,
    0x8000000000008003,
    0x8000000000008002,
    0x8000000000000080,
    0x000000000000800a,
    0x800000008000000a,
    0x8000000080008081,
    0x8000000000008080,
    0x0000000080000001,
    0x8000000080008008,
];

/// Rotation offsets and lane order of the combined rho and pi steps.
const RHO: [u32; 24] = [
    1, 3, 6, 10, 15, 21, 28, 36, 45, 55, 2, 14, 27, 41, 56, 8, 25, 43, 62, 18, 39, 61, 20, 44,
];
const PI: [usize; 24] = [
    10, 7, 11, 17, 18, 3, 5, 16, 8, 21, 24, 4, 15, 23, 19, 13, 12, 2, 20, 14, 22, 9, 6, 1,
];

fn keccak_f1600(state: &mut [u64; 25]) {
    for round_constant in ROUND_CONSTANTS {
        // theta
        let mut parity = [0u64; 5];
        for (x, column) in parity.iter_mut().enumerate() {
            *column = (0..5).fold(0, |acc, y| acc ^ state[x + 5 * y]);
        }
        for x in 0..5 {
            let mix = parity[(x + 4) % 5] ^ parity[(x + 1) % 5].rotate_left(1);
            for y in 0..5 {
                state[x + 5 * y] ^= mix;
            }
        }
        // rho and pi
        let mut carried = state[1];
        for (offset, target) in RHO.iter().zip(PI) {
            let next = state[target];
            state[target] = carried.rotate_left(*offset);
            carried = next;
        }
        // chi
        for y in 0..5 {
            let row: [u64; 5] = std::array::from_fn(|x| state[x + 5 * y]);
            for x in 0..5 {
                state[x + 5 * y] = row[x] ^ (!row[(x + 1) % 5] & row[(x + 2) % 5]);
            }
        }
        // iota
        state[0] ^= round_constant;
    }
}

#[cfg(test)]
mod tests {
    use super::{hex, sha3};

    #[test]
    fn sha3_matches_the_fips_202_vectors() {
        assert_eq!(
            hex(&sha3(b"", 224)),
            "6b4e03423667dbb73b6e15454f0eb1abd4597f9a1b078e3f5b5a6bc7"
        );
        assert_eq!(
            hex(&sha3(b"abc", 256)),
            "3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532"
        );
        assert_eq!(
            hex(&sha3(b"abc", 512)),
            "b751850b1a57168a5693cd924b6b096e08f621827444f70d884f5d0240d2712e\
             10e116e9192af3c91a7ec57647e3934057340b4cf408d5a56592f8274eec53f0"
        );
        // A message longer than one SHA3-384 block (104 bytes).
        let long = vec![b'a'; 200];
        assert_eq!(hex(&sha3(&long, 384)).len(), 96);
    }

    #[test]
    fn a_one_row_table_hashes_as_sqlite3_does() {
        // Case 00141: `CREATE TABLE t(x); INSERT INTO t VALUES(1); .sha3sum`.
        let query = b"SELECT * FROM \"t\" NOT INDEXED;";
        let mut content = format!("S{}:", query.len()).into_bytes();
        content.extend_from_slice(query);
        content.extend_from_slice(b"RI");
        content.extend_from_slice(&1i64.to_be_bytes());
        assert_eq!(
            hex(&sha3(&content, 224)),
            "536463bbf637986bfae5f52d889404ad048865c19215ef0c578cf8d3"
        );
    }
}
