//! Step rows retained by an index visibility recheck without loading or
//! decoding them a second time.

use super::*;

pub(super) fn step(
    rows: &mut std::vec::IntoIter<TableRow>,
    selection: &Option<Expr>,
    projection: &[SelectItem],
    bindings: &[Option<SqlValue>],
    seen: &mut usize,
    offset: usize,
    yielded: &mut usize,
    limit: usize,
    current_row: &mut Option<Vec<SqlValue>>,
) -> Result<bool> {
    for row in rows {
        let row = SqlRow::Table(row);
        if !selection_passes(selection, &row, bindings)? {
            continue;
        }
        *seen += 1;
        if *seen <= offset {
            continue;
        }
        if *yielded >= limit {
            *current_row = None;
            return Ok(true);
        }
        *current_row = Some(project_row(projection, &row, bindings)?);
        *yielded += 1;
        return Ok(false);
    }
    *current_row = None;
    Ok(true)
}
