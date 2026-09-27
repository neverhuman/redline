//! Integer and numeric affinity turn a REAL into an INTEGER only when the
//! value is a whole number below 2^63, so REAL 2^63 stays REAL as in
//! SQLite. (-2^63 still converts; see `real_is_exact_i64`.)

use std::sync::Arc;

use redlinedb_kernel::catalog::{Affinity, OwnedValue, apply_affinity};

const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;

fn coerce(value: OwnedValue, affinity: Affinity) -> OwnedValue {
    apply_affinity(value, affinity).unwrap()
}

fn text(v: &str) -> OwnedValue {
    OwnedValue::Text(Arc::from(v))
}

#[test]
fn real_two_pow_63_stays_real() {
    for affinity in [Affinity::Integer, Affinity::Numeric] {
        assert_eq!(
            coerce(OwnedValue::Real(TWO_POW_63), affinity),
            OwnedValue::Real(TWO_POW_63)
        );
        assert_eq!(
            coerce(text("9223372036854775808"), affinity),
            OwnedValue::Real(TWO_POW_63)
        );
        assert_eq!(
            coerce(text("9223372036854775807.0"), affinity),
            OwnedValue::Real(TWO_POW_63)
        );
    }
}

#[test]
fn whole_reals_inside_the_i64_limits_become_integers() {
    // The largest double below 2^63 and its negation.
    let below = 9_223_372_036_854_774_784.0;
    for affinity in [Affinity::Integer, Affinity::Numeric] {
        assert_eq!(
            coerce(OwnedValue::Real(below), affinity),
            OwnedValue::Integer(9_223_372_036_854_774_784)
        );
        assert_eq!(
            coerce(OwnedValue::Real(-below), affinity),
            OwnedValue::Integer(-9_223_372_036_854_774_784)
        );
        assert_eq!(
            coerce(OwnedValue::Real(5.0), affinity),
            OwnedValue::Integer(5)
        );
        assert_eq!(
            coerce(OwnedValue::Real(5.5), affinity),
            OwnedValue::Real(5.5)
        );
        assert_eq!(
            coerce(text("9223372036854775807"), affinity),
            OwnedValue::Integer(i64::MAX)
        );
        assert_eq!(
            coerce(text("-9223372036854775808"), affinity),
            OwnedValue::Integer(i64::MIN)
        );
    }
}
