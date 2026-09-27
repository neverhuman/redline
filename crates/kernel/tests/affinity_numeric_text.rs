//! Numeric affinity converts text only when it spells a number as SQLite
//! reads one. Rust's float parser also accepts `nan`, `inf` and `infinity`
//! in any case, which SQLite keeps as text: a REAL NaN compares equal to
//! every number, so letting one through turned such text into a wildcard.

use std::sync::Arc;

use redlinedb_kernel::catalog::{Affinity, OwnedValue, apply_affinity};

const NUMERIC: [Affinity; 3] = [Affinity::Integer, Affinity::Real, Affinity::Numeric];

fn text(v: &str) -> OwnedValue {
    OwnedValue::Text(Arc::from(v))
}

#[test]
fn nan_and_infinity_words_stay_text() {
    for word in [
        "nan",
        "NaN",
        "NAN",
        " nan ",
        "-nan",
        "+NaN",
        "inf",
        "Inf",
        "-inf",
        "+inf",
        "infinity",
        "Infinity",
        "-Infinity",
        "INFINITY",
    ] {
        for affinity in NUMERIC {
            assert_eq!(
                apply_affinity(text(word), affinity).unwrap(),
                text(word),
                "{word:?} under {affinity:?}"
            );
        }
    }
}

#[test]
fn digits_that_overflow_still_become_infinity() {
    for affinity in NUMERIC {
        assert_eq!(
            apply_affinity(text("1e999"), affinity).unwrap(),
            OwnedValue::Real(f64::INFINITY),
            "{affinity:?}"
        );
        assert_eq!(
            apply_affinity(text("-1e999"), affinity).unwrap(),
            OwnedValue::Real(f64::NEG_INFINITY),
            "{affinity:?}"
        );
    }
    assert_eq!(
        apply_affinity(text(" 5 "), Affinity::Integer).unwrap(),
        OwnedValue::Integer(5)
    );
    assert_eq!(
        apply_affinity(text(".5"), Affinity::Numeric).unwrap(),
        OwnedValue::Real(0.5)
    );
}
