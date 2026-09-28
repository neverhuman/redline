//! Aggregate-OVER accumulator: SUM / COUNT / AVG / MIN / MAX / TOTAL
//! evaluated over a window-frame slice.

use std::cmp::Ordering;
use std::sync::Arc;

use crate::error::Result;
use crate::numeric::SumAcc;
use crate::value::{SqlValue, compare_values};

#[derive(Clone)]
pub(super) struct Accumulator {
    kind: AccumulatorKind,
    count: i64,
    /// SUM / TOTAL / AVG: SQLite's sumStep accumulator, shared with the
    /// grouped-aggregate routes.
    sum: SumAcc,
    min: Option<SqlValue>,
    max: Option<SqlValue>,
}

#[derive(Clone, Copy)]
enum AccumulatorKind {
    Count,
    Sum,
    Total,
    Avg,
    Min,
    Max,
    Unknown,
}

impl AccumulatorKind {
    fn from_name(name: &str) -> Self {
        match name {
            "count" => Self::Count,
            "sum" => Self::Sum,
            "total" => Self::Total,
            "avg" => Self::Avg,
            "min" => Self::Min,
            "max" => Self::Max,
            _ => Self::Unknown,
        }
    }
}

impl Accumulator {
    pub(super) fn new(name: &str) -> Self {
        Self {
            kind: AccumulatorKind::from_name(name),
            count: 0,
            sum: SumAcc::new(),
            min: None,
            max: None,
        }
    }

    pub(super) fn push(&mut self, value: SqlValue) {
        match value {
            SqlValue::Null => {}
            ref v => {
                self.count += 1;
                self.sum.step_value(v);
                self.update_min_max(v);
            }
        }
    }

    fn avg_value(&self) -> SqlValue {
        // Postgres `avg` of integers is numeric, shown with 16 fractional
        // digits (`15.0000000000000000`). SQLite keeps a float.
        if let Some(int_sum) = self.sum.exact_int()
            && crate::value::postgres_result_dialect()
        {
            return pg_integer_avg(int_sum, self.count);
        }
        self.sum.avg()
    }

    fn update_min_max(&mut self, v: &SqlValue) {
        match &self.min {
            None => self.min = Some(v.clone()),
            Some(cur) if compare_values(v, cur) == Ordering::Less => {
                self.min = Some(v.clone());
            }
            _ => {}
        }
        match &self.max {
            None => self.max = Some(v.clone()),
            Some(cur) if compare_values(v, cur) == Ordering::Greater => {
                self.max = Some(v.clone());
            }
            _ => {}
        }
    }

    /// The aggregate over every pushed value. `sum()` raises `integer
    /// overflow` like SQLite instead of answering REAL; TOTAL and AVG stay
    /// REAL.
    pub(super) fn finalize(self) -> Result<SqlValue> {
        self.value()
    }

    pub(super) fn value(&self) -> Result<SqlValue> {
        Ok(match self.kind {
            AccumulatorKind::Count => SqlValue::Integer(self.count),
            AccumulatorKind::Sum => self.sum.sum()?,
            AccumulatorKind::Total => self.sum.total(),
            AccumulatorKind::Avg => self.avg_value(),
            AccumulatorKind::Min => self.min.clone().unwrap_or(SqlValue::Null),
            AccumulatorKind::Max => self.max.clone().unwrap_or(SqlValue::Null),
            AccumulatorKind::Unknown => SqlValue::Null,
        })
    }
}

/// Integer `avg` as Postgres numeric text. Truncates extra digits, which
/// matches `avg` of `(1),(1),(2)` → `1.3333333333333333`.
fn pg_integer_avg(sum: i64, count: i64) -> SqlValue {
    if count == 0 {
        return SqlValue::Null;
    }
    let negative = (sum < 0) ^ (count < 0);
    let sum = sum.unsigned_abs();
    let count = count.unsigned_abs();
    let whole = sum / count;
    let frac = (sum % count) as u128 * 10u128.pow(16) / count as u128;
    let body = format!("{whole}.{frac:016}");
    let text = if negative && (whole != 0 || frac != 0) {
        format!("-{body}")
    } else {
        body
    };
    SqlValue::Text(Arc::from(text))
}
