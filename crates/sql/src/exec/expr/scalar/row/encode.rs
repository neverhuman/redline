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
    let refs = values.iter().map(|v| v.as_ref()).collect::<Vec<_>>();
    encode_record(&refs, &mut out).map_err(|_| Error::DatatypeMismatch)?;
    Ok(out)
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
