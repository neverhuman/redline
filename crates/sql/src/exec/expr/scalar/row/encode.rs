use super::super::*;

pub(crate) fn unique_key_bytes(
    table_id: u64,
    constraint_id: u64,
    values: &[SqlValue],
) -> Result<Vec<u8>> {
    // Phase 4.3: hint capacity. Table-id + constraint-id are 16 bytes;
    // the record encoding adds 1-9 bytes header + ~5 bytes per value
    // depending on type. 32 bytes per value is a safe upper bound for
    // most typical INTEGER/REAL/short-TEXT keys, and reserving here
    // eliminates the grow-by-doubling reallocation cascade.
    let mut out = Vec::with_capacity(16 + values.len() * 32);
    out.extend_from_slice(&table_id.to_le_bytes());
    out.extend_from_slice(&constraint_id.to_le_bytes());
    let canonical = values.iter().map(lock_key_value).collect::<Vec<_>>();
    let refs = canonical.iter().map(|v| v.as_ref()).collect::<Vec<_>>();
    encode_record(&refs, &mut out).map_err(|_| Error::DatatypeMismatch)?;
    Ok(out)
}

/// The value a unique-key lock encodes for `value`. The conflict check
/// compares keys with `compare_values`, so every pair of values it calls
/// equal must take one lock; the mapping may over-share (writers of keys
/// that share a lock only wait for each other), never under-share.
///
/// * `compare_values` compares an INTEGER with a REAL as doubles, so 1 and
///   1.0, or i64::MAX and REAL 2^63, are one key: every number becomes that
///   double, with -0.0 as 0.0.
/// * Every NaN payload compares equal, so every NaN takes one lock.
/// * While `::citext` markers are honoured (a Postgres-dialect statement
///   after `CREATE EXTENSION citext`), a marked text equals any spelling of
///   it with or without the marker, so text locks on its unmarked ASCII
///   lower-case form. Without the marker texts compare by bytes and lock
///   as they are.
fn lock_key_value(value: &SqlValue) -> SqlValue {
    match value {
        SqlValue::Integer(v) => SqlValue::Real(*v as f64),
        SqlValue::Real(v) if v.is_nan() => SqlValue::Real(f64::NAN),
        SqlValue::Real(v) => SqlValue::Real(*v + 0.0),
        SqlValue::Text(text) if crate::value::citext_marker_active() => {
            let unmarked = text.strip_prefix('\u{E000}').unwrap_or(text);
            SqlValue::Text(Arc::from(unmarked.to_ascii_lowercase()))
        }
        other => other.clone(),
    }
}

pub(crate) fn key_values_equal(left: &[SqlValue], right: &[SqlValue]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right.iter())
            .all(|(a, b)| compare_values(a, b) == Ordering::Equal)
}

pub(crate) fn encode_sql_row(table_id: u64, values: &[SqlValue]) -> Result<Vec<u8>> {
    // Phase 4.3: hint capacity. Called per-row in DML
    // (INSERT/UPDATE/DELETE) heap encoding.
    let mut out = Vec::with_capacity(16 + values.len() * 32);
    let mut refs = Vec::with_capacity(values.len() + 1);
    refs.push(ValueRef::Integer(table_id as i64));
    refs.extend(values.iter().map(|value| value.as_ref()));
    encode_record(&refs, &mut out).map_err(|_| Error::DatatypeMismatch)?;
    Ok(out)
}

/// Table id stored in column 0. Does not allocate the other column values.
pub(crate) fn sql_row_table_id(bytes: &[u8]) -> Result<Option<u64>> {
    let record = RecordRef::new(bytes).map_err(|_| Error::DatatypeMismatch)?;
    let mut scratch = RecordScratch::default();
    record
        .decode_into(&mut scratch)
        .map_err(|_| Error::DatatypeMismatch)?;
    match record
        .value_at(&scratch, 0)
        .map_err(|_| Error::DatatypeMismatch)?
    {
        ValueRef::Integer(v) => Ok(Some(v as u64)),
        _ => Err(Error::DatatypeMismatch),
    }
}

/// Materialise only `ordinals` (table columns, not counting the stored
/// table id); other columns stay null. Returns `None` when the record ends
/// before one of `ordinals`: a row written before ALTER TABLE ADD COLUMN
/// holds that column's DEFAULT, which only the full row load fills in.
/// Callers that need every column use [`decode_sql_row`].
pub(crate) fn decode_sql_key_columns(
    bytes: &[u8],
    ordinals: &[usize],
    width: usize,
) -> Result<Option<(u64, Vec<SqlValue>)>> {
    let record = RecordRef::new(bytes).map_err(|_| Error::DatatypeMismatch)?;
    let mut scratch = RecordScratch::default();
    record
        .decode_into(&mut scratch)
        .map_err(|_| Error::DatatypeMismatch)?;
    let table_id = match record
        .value_at(&scratch, 0)
        .map_err(|_| Error::DatatypeMismatch)?
    {
        ValueRef::Integer(v) => v as u64,
        _ => return Err(Error::DatatypeMismatch),
    };
    let mut values = vec![SqlValue::Null; width];
    let columns = record.column_count().map_err(|_| Error::DatatypeMismatch)?;
    for ordinal in ordinals {
        let slot = ordinal.saturating_add(1);
        if slot >= columns {
            return Ok(None);
        }
        if *ordinal >= width {
            continue;
        }
        values[*ordinal] = record
            .value_at(&scratch, slot)
            .map_err(|_| Error::DatatypeMismatch)?
            .to_owned();
    }
    Ok(Some((table_id, values)))
}

pub(crate) fn decode_sql_row(bytes: &[u8]) -> Result<Option<(u64, Vec<SqlValue>)>> {
    let record = RecordRef::new(bytes).map_err(|_| Error::DatatypeMismatch)?;
    let mut scratch = RecordScratch::default();
    record
        .decode_into(&mut scratch)
        .map_err(|_| Error::DatatypeMismatch)?;
    let mut values = Vec::new();
    let table_id = match record
        .value_at(&scratch, 0)
        .map_err(|_| Error::DatatypeMismatch)?
    {
        ValueRef::Integer(v) => v as u64,
        _ => return Err(Error::DatatypeMismatch),
    };
    for idx in 1..record.column_count().map_err(|_| Error::DatatypeMismatch)? {
        let value = record
            .value_at(&scratch, idx)
            .map_err(|_| Error::DatatypeMismatch)?;
        values.push(value.to_owned());
    }
    Ok(Some((table_id, values)))
}

#[cfg(test)]
mod lock_key_tests {
    use super::*;
    use crate::connection::Dialect;
    use crate::value::DialectScope;

    fn key(values: &[SqlValue]) -> Vec<u8> {
        unique_key_bytes(7, 3, values).expect("key")
    }

    fn text(v: &str) -> SqlValue {
        SqlValue::Text(Arc::from(v))
    }

    #[test]
    fn every_nan_takes_one_lock() {
        let quiet = f64::NAN;
        let negative = -f64::NAN;
        let payload = f64::from_bits(0x7ff8_0000_0000_0042);
        assert!(quiet.is_nan() && negative.is_nan() && payload.is_nan());
        assert_ne!(quiet.to_bits(), negative.to_bits());
        let want = key(&[SqlValue::Real(quiet)]);
        assert_eq!(key(&[SqlValue::Real(negative)]), want);
        assert_eq!(key(&[SqlValue::Real(payload)]), want);
        assert_eq!(
            key(&[SqlValue::Integer(1)]),
            key(&[SqlValue::Real(1.0)]),
            "INTEGER 1 and REAL 1.0 still share a lock"
        );
    }

    #[test]
    fn citext_spellings_take_one_lock_only_while_citext_is_honoured() {
        // Outside a Postgres-dialect statement the marker is a character.
        assert_ne!(key(&[text("\u{E000}ABC")]), key(&[text("abc")]));
        crate::value::enable_citext_marker();
        {
            let _sqlite = DialectScope::for_dialect(Dialect::Sqlite);
            assert_ne!(key(&[text("\u{E000}ABC")]), key(&[text("abc")]));
            assert_ne!(key(&[text("ABC")]), key(&[text("abc")]));
        }
        let _postgres = DialectScope::for_dialect(Dialect::PostgresSubset);
        let want = key(&[text("abc")]);
        assert_eq!(key(&[text("\u{E000}ABC")]), want);
        assert_eq!(key(&[text("\u{E000}abc")]), want);
        assert_eq!(key(&[text("ABC")]), want);
    }
}
