//! Key affinity for foreign-key comparisons, as SQLite applies it.
//!
//! - Child to parent (`fkLookupParent`): the child key takes the parent key
//!   column's affinity before the parent row is looked up, so a TEXT '5'
//!   finds an INTEGER or REAL parent key 5.
//! - Parent to child (`fkScanChildren`): SQLite scans the child table with
//!   `parent_key = child_column`. When either column has numeric affinity,
//!   numeric text on either side is compared as its number.
//!
//! Collations are not applied here (a NOCASE key still compares
//! binary), as before.

use std::cmp::Ordering;
use std::sync::Arc;

use crate::value::{Affinity, SqlValue, apply_affinity, compare_values};

/// `value` as a column with `affinity` would store it.
pub(super) fn with_column_affinity(value: &SqlValue, affinity: Affinity) -> SqlValue {
    // Same REAL-to-TEXT formatting as `apply_row_affinity`.
    if let (Affinity::Text, SqlValue::Real(v)) = (affinity, value) {
        return SqlValue::Text(Arc::from(crate::format_real_sqlite(*v)));
    }
    apply_affinity(value.clone(), affinity).unwrap_or_else(|_| value.clone())
}

fn is_numeric(affinity: Affinity) -> bool {
    matches!(
        affinity,
        Affinity::Integer | Affinity::Real | Affinity::Numeric
    )
}

fn numeric_text(value: &SqlValue) -> SqlValue {
    match value {
        SqlValue::Text(_) => with_column_affinity(value, Affinity::Numeric),
        other => other.clone(),
    }
}

/// True when the parent key value equals the child column value under the
/// comparison affinity of `parent_key = child_column`. NULL never matches.
pub(super) fn parent_child_equal(
    parent: &SqlValue,
    parent_affinity: Affinity,
    child: &SqlValue,
    child_affinity: Affinity,
) -> bool {
    if matches!(parent, SqlValue::Null) || matches!(child, SqlValue::Null) {
        return false;
    }
    if is_numeric(parent_affinity) || is_numeric(child_affinity) {
        return compare_values(&numeric_text(parent), &numeric_text(child)) == Ordering::Equal;
    }
    compare_values(parent, child) == Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(v: &str) -> SqlValue {
        SqlValue::Text(Arc::from(v))
    }

    #[test]
    fn child_key_takes_parent_affinity() {
        assert_eq!(
            with_column_affinity(&text("5"), Affinity::Integer),
            SqlValue::Integer(5)
        );
        assert_eq!(
            with_column_affinity(&text("5"), Affinity::Real),
            SqlValue::Real(5.0)
        );
        assert_eq!(
            with_column_affinity(&SqlValue::Real(5.0), Affinity::Text),
            text("5.0")
        );
        assert_eq!(with_column_affinity(&text("5"), Affinity::Blob), text("5"));
    }

    #[test]
    fn numeric_side_makes_numeric_text_equal() {
        let int = SqlValue::Integer(5);
        assert!(parent_child_equal(
            &int,
            Affinity::Integer,
            &text("5"),
            Affinity::Text
        ));
        assert!(parent_child_equal(
            &text("5"),
            Affinity::Text,
            &int,
            Affinity::Integer
        ));
        assert!(!parent_child_equal(
            &int,
            Affinity::Blob,
            &text("5"),
            Affinity::Text
        ));
        assert!(!parent_child_equal(
            &SqlValue::Null,
            Affinity::Integer,
            &SqlValue::Null,
            Affinity::Integer
        ));
    }
}
