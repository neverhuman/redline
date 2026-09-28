//! Compound-SELECT set operations: `UNION`, `INTERSECT`, `EXCEPT`.
//!
//! `UNION ALL` keeps its existing fast path in [`SelectSource::CompoundAll`];
//! the three deduplicating operations here materialise both branches into
//! row vectors, then combine according to [`CompoundSetOp`] with hash dedup.
//! Rows are equal when SQLite's comparison says so, which the
//! [`super::sql_equiv`] keys encode: INTEGER 1 and REAL 1.0 are one row,
//! TEXT '1' and INTEGER 1 are two. Text compares under each column's
//! collation (Q5-10): the key is taken over the text as the collation folds
//! it, while the surviving row keeps its stored text.
//!
//! Column count compatibility is checked at execution time; type-class
//! compatibility uses SQLite-compatible "any-type-fits-any-cell" semantics
//! since storage classes are dynamic.

use crate::collation::Collation;
use crate::connection::Connection;
use crate::error::{Error, Result};
use crate::exec::expr::coerce::{collation_keyed_row, plan_output_collations};
use crate::statement::{CompoundSetOp, SelectPlan};
use crate::value::SqlValue;

use super::materialize_select_plan_rows;
use super::sql_equiv::equiv_key_into;

/// The [`super::sql_equiv`] key of `row` with each column's text folded by
/// its collation, so rows equal under the collations share a key.
fn key_into(row: &[SqlValue], collations: &[Option<Collation>], out: &mut Vec<u8>) {
    if collations.iter().all(Option::is_none) {
        equiv_key_into(row, out);
    } else {
        equiv_key_into(&collation_keyed_row(row, collations), out);
    }
}

/// Rows deduplicated by their key: the first of equal rows survives, in
/// its first position.
fn dedup_rows(rows: Vec<Vec<SqlValue>>, collations: &[Option<Collation>]) -> Vec<Vec<SqlValue>> {
    let mut seen: ahash::AHashSet<Vec<u8>> = ahash::AHashSet::with_capacity(rows.len());
    let mut key = Vec::new();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        key_into(&row, collations, &mut key);
        if !seen.contains(key.as_slice()) {
            seen.insert(key.clone());
            out.push(row);
        }
    }
    out
}

/// `left UNION right`, as SQLite 3.53 answers it: among equal rows of one
/// operand the first survives, and on a tie between the operands the right
/// operand's row replaces the left one in place (`SELECT 1 UNION SELECT
/// 1.0` is REAL 1.0, `SELECT 1.0 UNION SELECT 1` is INTEGER 1).
fn union_rows(
    left: Vec<Vec<SqlValue>>,
    right: Vec<Vec<SqlValue>>,
    collations: &[Option<Collation>],
) -> Vec<Vec<SqlValue>> {
    let mut slot_by_key: ahash::AHashMap<Vec<u8>, usize> =
        ahash::AHashMap::with_capacity(left.len() + right.len());
    let mut out: Vec<Vec<SqlValue>> = Vec::with_capacity(left.len() + right.len());
    let mut from_right: Vec<bool> = Vec::with_capacity(left.len() + right.len());
    let mut key = Vec::new();
    for (row, is_right) in left
        .into_iter()
        .map(|row| (row, false))
        .chain(right.into_iter().map(|row| (row, true)))
    {
        key_into(&row, collations, &mut key);
        match slot_by_key.get(key.as_slice()) {
            Some(&slot) => {
                if is_right && !from_right[slot] {
                    out[slot] = row;
                    from_right[slot] = true;
                }
            }
            None => {
                slot_by_key.insert(key.clone(), out.len());
                out.push(row);
                from_right.push(is_right);
            }
        }
    }
    out
}

fn check_arity(left: &[Vec<SqlValue>], right: &[Vec<SqlValue>]) -> Result<()> {
    let lw = left.first().map(|r| r.len());
    let rw = right.first().map(|r| r.len());
    match (lw, rw) {
        (Some(a), Some(b)) if a != b => Err(Error::UnsupportedSql(format!(
            "SELECTs to the left and right of a set operator do not have the same number of result columns ({a} vs {b})"
        ))),
        _ => Ok(()),
    }
}

pub(crate) fn collect_compound_set_rows(
    conn: &Connection,
    op: CompoundSetOp,
    branches: &[SelectPlan],
    bindings: &[Option<SqlValue>],
) -> Result<Vec<Vec<SqlValue>>> {
    if branches.is_empty() {
        return Ok(Vec::new());
    }
    // Each result column compares under the collation of the left-most
    // branch that gives it one (SQLite's multiSelectCollSeq).
    let mut collations: Vec<Option<Collation>> = Vec::new();
    for branch in branches {
        for (idx, collation) in plan_output_collations(branch).into_iter().enumerate() {
            if collations.len() <= idx {
                collations.resize(idx + 1, None);
            }
            if collations[idx].is_none() {
                collations[idx] = collation;
            }
        }
    }
    // Materialise the first branch and then fold the rest in left-to-right.
    // Every `combine_two` result is already free of equal rows.
    let mut accum = dedup_rows(
        materialize_select_plan_rows(conn, &branches[0], bindings)?,
        &collations,
    );
    for branch in &branches[1..] {
        let next = materialize_select_plan_rows(conn, branch, bindings)?;
        check_arity(&accum, &next)?;
        accum = combine_two(op, accum, next, &collations);
    }
    Ok(accum)
}

/// Combine two operands. `left` holds no two equal rows. INTERSECT and
/// EXCEPT keep left rows; UNION follows [`union_rows`].
fn combine_two(
    op: CompoundSetOp,
    left: Vec<Vec<SqlValue>>,
    right: Vec<Vec<SqlValue>>,
    collations: &[Option<Collation>],
) -> Vec<Vec<SqlValue>> {
    match op {
        CompoundSetOp::UnionDistinct => union_rows(left, right, collations),
        CompoundSetOp::Intersect | CompoundSetOp::Except => {
            let keep_matches = matches!(op, CompoundSetOp::Intersect);
            let mut key = Vec::new();
            let r_keys: ahash::AHashSet<Vec<u8>> = right
                .iter()
                .map(|r| {
                    key_into(r, collations, &mut key);
                    key.clone()
                })
                .collect();
            left.into_iter()
                .filter(|row| {
                    key_into(row, collations, &mut key);
                    r_keys.contains(key.as_slice()) == keep_matches
                })
                .collect()
        }
    }
}
