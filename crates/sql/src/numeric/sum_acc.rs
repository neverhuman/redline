//! SQLite's `sum()` / `total()` / `avg()` accumulator (`SumCtx` with
//! `sumStep`, `sumFinalize`, `totalFinalize` and `avgFinalize` in
//! SQLite's func.c), shared by every aggregate route so the answer does
//! not depend on which executor path ran.
//!
//! While every input is an INTEGER the sum is kept exactly with checked
//! addition. The first non-INTEGER input, or the first overflow, switches
//! to a Kahan-Babuska-Neumaier compensated REAL sum. `sum()` then raises
//! `integer overflow` when an overflow happened and no non-INTEGER value
//! arrived; `total()` and `avg()` always answer REAL.
//!
//! This file depends only on `crate::error`, `crate::value` and the kernel
//! so `tests/morsel_hash_agg.rs` can re-include it next to the morsel
//! aggregator.

use redlinedb_kernel::catalog::{ValueRef, sqlite_numeric_prefix};

use crate::error::{Error, Result};
use crate::value::SqlValue;

/// |i| at or above 2^52 is split before the compensated add, as SQLite's
/// `kahanBabuskaNeumaierStepInt64` does, so no integer bits are lost.
const KBN_SPLIT: i64 = 4_503_599_627_370_496;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SumAcc {
    r_sum: f64,
    r_err: f64,
    i_sum: i64,
    cnt: i64,
    /// A non-INTEGER input or an overflow switched to the REAL sum.
    approx: bool,
    /// The exact INTEGER sum overflowed while every input was an INTEGER.
    overflowed: bool,
    /// A non-INTEGER input arrived; SQLite then drops the overflow error.
    saw_non_int: bool,
}

/// Flag bits for [`SumAcc::to_parts`] / [`SumAcc::from_parts`].
const FLAG_APPROX: i64 = 1;
const FLAG_OVERFLOWED: i64 = 2;
const FLAG_SAW_NON_INT: i64 = 4;

impl SumAcc {
    pub fn new() -> Self {
        Self::default()
    }

    /// The exact INTEGER sum while no REAL arithmetic was needed.
    pub fn exact_int(&self) -> Option<i64> {
        (!self.approx).then_some(self.i_sum)
    }

    pub fn step_int(&mut self, v: i64) {
        self.cnt += 1;
        if self.approx {
            self.kbn_step_int(v);
            return;
        }
        match self.i_sum.checked_add(v) {
            Some(sum) => self.i_sum = sum,
            None => {
                self.overflowed = true;
                self.start_approx();
                self.kbn_step_int(v);
            }
        }
    }

    pub fn step_real(&mut self, v: f64) {
        self.cnt += 1;
        self.saw_non_int = true;
        if !self.approx {
            self.start_approx();
        }
        self.kbn_step(v);
    }

    /// One input value. NULL is skipped. TEXT that spells an INTEGER counts
    /// as INTEGER (`sqlite3_value_numeric_type`); any other TEXT or BLOB is
    /// a REAL read from its numeric prefix (`'7abc'` is 7.0, `'abc'` 0.0).
    pub fn step_value(&mut self, value: &SqlValue) {
        match value {
            SqlValue::Null => {}
            SqlValue::Integer(v) => self.step_int(*v),
            SqlValue::Real(v) => self.step_real(*v),
            SqlValue::Text(v) => self.step_text(v.as_bytes()),
            SqlValue::Blob(v) => self.step_real(sqlite_numeric_prefix(v)),
        }
    }

    /// [`SumAcc::step_value`] for a borrowed value.
    pub fn step_ref(&mut self, value: ValueRef<'_>) {
        match value {
            ValueRef::Null => {}
            ValueRef::Integer(v) => self.step_int(v),
            ValueRef::Real(v) => self.step_real(v),
            ValueRef::Text(v) => self.step_text(v.as_bytes()),
            ValueRef::Blob(v) => self.step_real(sqlite_numeric_prefix(v)),
        }
    }

    /// SQLite's `sumInverse`: remove a value that leaves a sliding window
    /// frame. While the sum is exact the INTEGER is subtracted with an
    /// overflow check; an overflow there marks the sum approximate and
    /// overflowed without seeding the REAL sum, exactly as SQLite does.
    pub fn inverse_value(&mut self, value: &SqlValue) {
        let (int, real) = match value {
            SqlValue::Null => return,
            SqlValue::Integer(v) => (Some(*v), *v as f64),
            SqlValue::Real(v) => (None, *v),
            SqlValue::Text(v) => match text_integer(v.as_bytes()) {
                Some(i) => (Some(i), i as f64),
                None => (None, sqlite_numeric_prefix(v.as_bytes())),
            },
            SqlValue::Blob(v) => (None, sqlite_numeric_prefix(v)),
        };
        self.cnt -= 1;
        if !self.approx {
            // Only INTEGER inputs reach an exact sum; `sqlite3_value_int64`
            // of anything else would be its truncation.
            let v = int.unwrap_or(real as i64);
            match self.i_sum.checked_sub(v) {
                Some(diff) => self.i_sum = diff,
                None => {
                    self.overflowed = true;
                    self.approx = true;
                }
            }
        } else if let Some(v) = int {
            if v != i64::MIN {
                self.kbn_step_int(-v);
            } else {
                self.kbn_step_int(i64::MAX);
                self.kbn_step_int(1);
            }
        } else {
            self.kbn_step(-real);
        }
    }

    fn step_text(&mut self, bytes: &[u8]) {
        match text_integer(bytes) {
            Some(v) => self.step_int(v),
            None => self.step_real(sqlite_numeric_prefix(bytes)),
        }
    }

    /// `sum()`: NULL for no input, INTEGER while exact, the `integer
    /// overflow` error after an all-INTEGER overflow, otherwise REAL.
    pub fn sum(&self) -> Result<SqlValue> {
        if self.cnt == 0 {
            return Ok(SqlValue::Null);
        }
        if !self.approx {
            return Ok(SqlValue::Integer(self.i_sum));
        }
        if self.overflowed && !self.saw_non_int {
            return Err(Error::IntegerOverflow);
        }
        Ok(real_result(self.approx_value()))
    }

    /// `total()`: always REAL, 0.0 for no input.
    pub fn total(&self) -> SqlValue {
        real_result(self.real_value())
    }

    /// `avg()`: NULL for no input, else REAL.
    pub fn avg(&self) -> SqlValue {
        if self.cnt == 0 {
            return SqlValue::Null;
        }
        real_result(self.real_value() / self.cnt as f64)
    }

    /// Fold a partial accumulator that saw the inputs after this one. Used
    /// when a spilled hash aggregate merges its partial states. Exact for
    /// the result value; an overflow is detected from the two partial sums,
    /// so an overflow that happened only inside `later`'s own running sum
    /// and cancelled out again is seen as one there.
    pub fn merge(&mut self, later: &SumAcc) {
        if later.cnt == 0 {
            return;
        }
        if self.cnt == 0 {
            *self = *later;
            return;
        }
        self.cnt += later.cnt;
        self.saw_non_int |= later.saw_non_int;
        self.overflowed |= later.overflowed;
        if !self.approx && !later.approx {
            match self.i_sum.checked_add(later.i_sum) {
                Some(sum) => self.i_sum = sum,
                None => {
                    self.overflowed = true;
                    self.start_approx();
                    self.kbn_step_int(later.i_sum);
                }
            }
            return;
        }
        if !self.approx {
            self.start_approx();
        }
        if later.approx {
            self.kbn_step(later.r_sum);
            self.r_err += later.r_err;
        } else {
            self.kbn_step_int(later.i_sum);
        }
    }

    /// Serialise for a spill file: (count, int sum, real sum, error, flags).
    pub fn to_parts(self) -> (i64, i64, f64, f64, i64) {
        let mut flags = 0;
        if self.approx {
            flags |= FLAG_APPROX;
        }
        if self.overflowed {
            flags |= FLAG_OVERFLOWED;
        }
        if self.saw_non_int {
            flags |= FLAG_SAW_NON_INT;
        }
        (self.cnt, self.i_sum, self.r_sum, self.r_err, flags)
    }

    pub fn from_parts(cnt: i64, i_sum: i64, r_sum: f64, r_err: f64, flags: i64) -> Self {
        Self {
            r_sum,
            r_err,
            i_sum,
            cnt,
            approx: flags & FLAG_APPROX != 0,
            overflowed: flags & FLAG_OVERFLOWED != 0,
            saw_non_int: flags & FLAG_SAW_NON_INT != 0,
        }
    }

    fn approx_value(&self) -> f64 {
        if self.r_err.is_finite() {
            self.r_sum + self.r_err
        } else {
            self.r_sum
        }
    }

    fn real_value(&self) -> f64 {
        if self.approx {
            self.approx_value()
        } else {
            self.i_sum as f64
        }
    }

    /// `kahanBabuskaNeumaierInit`: seed the REAL sum from the INTEGER sum.
    fn start_approx(&mut self) {
        self.approx = true;
        let v = self.i_sum;
        if v <= -KBN_SPLIT || v >= KBN_SPLIT {
            let small = v % 16384;
            self.r_sum = (v - small) as f64;
            self.r_err = small as f64;
        } else {
            self.r_sum = v as f64;
            self.r_err = 0.0;
        }
    }

    /// `kahanBabuskaNeumaierStep`.
    fn kbn_step(&mut self, r: f64) {
        let s = self.r_sum;
        let t = s + r;
        if s.abs() > r.abs() {
            self.r_err += (s - t) + r;
        } else {
            self.r_err += (r - t) + s;
        }
        self.r_sum = t;
    }

    /// `kahanBabuskaNeumaierStepInt64`.
    fn kbn_step_int(&mut self, v: i64) {
        if v <= -KBN_SPLIT || v >= KBN_SPLIT {
            let small = v % 16384;
            self.kbn_step((v - small) as f64);
            self.kbn_step(small as f64);
        } else {
            self.kbn_step(v as f64);
        }
    }
}

/// A REAL aggregate result; NaN is NULL, as `sqlite3_result_double` stores it.
fn real_result(v: f64) -> SqlValue {
    if v.is_nan() {
        SqlValue::Null
    } else {
        SqlValue::Real(v)
    }
}

/// TEXT that spells an INTEGER inside i64 (surrounding whitespace allowed).
fn text_integer(bytes: &[u8]) -> Option<i64> {
    let text = std::str::from_utf8(bytes).ok()?;
    let trimmed =
        text.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r'));
    trimmed.parse::<i64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sum_of(values: &[SqlValue]) -> SumAcc {
        let mut acc = SumAcc::new();
        for v in values {
            acc.step_value(v);
        }
        acc
    }

    #[test]
    fn integer_overflow_errors_even_when_it_cancels() {
        let acc = sum_of(&[
            SqlValue::Integer(i64::MAX),
            SqlValue::Integer(1),
            SqlValue::Integer(-1),
        ]);
        assert_eq!(acc.sum(), Err(Error::IntegerOverflow));
        let acc = sum_of(&[
            SqlValue::Integer(-1),
            SqlValue::Integer(i64::MAX),
            SqlValue::Integer(1),
        ]);
        assert_eq!(acc.sum(), Ok(SqlValue::Integer(i64::MAX)));
    }

    #[test]
    fn a_real_input_keeps_the_sum_approximate() {
        let before = sum_of(&[
            SqlValue::Real(1.5),
            SqlValue::Integer(i64::MAX),
            SqlValue::Integer(1),
        ]);
        assert!(matches!(before.sum(), Ok(SqlValue::Real(_))));
        let after = sum_of(&[
            SqlValue::Integer(i64::MAX),
            SqlValue::Integer(1),
            SqlValue::Real(0.5),
        ]);
        assert!(matches!(after.sum(), Ok(SqlValue::Real(_))));
    }

    #[test]
    fn total_and_avg_stay_real_after_overflow() {
        let acc = sum_of(&[SqlValue::Integer(i64::MAX), SqlValue::Integer(1)]);
        assert_eq!(acc.total(), SqlValue::Real(9_223_372_036_854_775_808.0));
        assert_eq!(acc.avg(), SqlValue::Real(4_611_686_018_427_387_904.0));
        assert_eq!(SumAcc::new().total(), SqlValue::Real(0.0));
        assert_eq!(SumAcc::new().avg(), SqlValue::Null);
        assert_eq!(SumAcc::new().sum(), Ok(SqlValue::Null));
    }

    #[test]
    fn compensated_real_sum_matches_sqlite() {
        let acc = sum_of(&[
            SqlValue::Real(0.2),
            SqlValue::Real(1e100),
            SqlValue::Real(-1e100),
        ]);
        assert_eq!(acc.sum(), Ok(SqlValue::Real(0.2)));
    }

    #[test]
    fn merge_matches_one_pass_for_split_inputs() {
        let values = [
            SqlValue::Integer(i64::MAX - 5),
            SqlValue::Integer(3),
            SqlValue::Integer(4),
        ];
        let whole = sum_of(&values);
        let mut split = sum_of(&values[..1]);
        split.merge(&sum_of(&values[1..]));
        assert_eq!(whole.sum(), Err(Error::IntegerOverflow));
        assert_eq!(split.sum(), Err(Error::IntegerOverflow));
        let (cnt, i_sum, r_sum, r_err, flags) = split.to_parts();
        assert_eq!(SumAcc::from_parts(cnt, i_sum, r_sum, r_err, flags), split);
    }
}
