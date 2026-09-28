//! `sum()` / `total()` / `avg()` over a sliding window frame, computed the
//! way SQLite computes it: one accumulator per partition that adds rows as
//! its end cursor passes them (`xStep`) and removes them as its start
//! cursor passes them (`xInverse`). Recomputing every frame from scratch
//! answers differently once an overflow or a REAL value has passed through
//! the frame, because SQLite's accumulator stays approximate afterwards,
//! and SQLite detects an INTEGER overflow at the moment its schedule adds
//! the row, not per frame.
//!
//! Frames that start at UNBOUNDED PRECEDING never remove a row, so the
//! per-frame recomputation already matches; frames with an EXCLUDE clause
//! are recomputed per row by SQLite too. Both stay on the generic path.
//! ROWS frames follow the operation order of SQLite's `windowCodeStep`;
//! RANGE and GROUPS frames use the same accumulator, removing the rows
//! that left the frame before adding the rows that entered it.

use sqlparser::ast::{Expr, WindowFrameUnits};

use super::frame::{ExcludeMode, ResolvedBound, ResolvedFrame, frame_bounds};
use super::{CachedWindowPartition, SqlRow, eval_scalar};
use crate::error::Result;
use crate::numeric::SumAcc;
use crate::value::SqlValue;

#[derive(Clone, Copy)]
enum SumKind {
    Sum,
    Total,
    Avg,
}

/// SQLite's step / return / inverse order for a ROWS frame, from the three
/// branches of `windowCodeStep`.
#[derive(Clone, Copy)]
enum RowsSchedule {
    /// `a FOLLOWING AND b FOLLOWING`, `1 <= a <= b`: each input row steps,
    /// returns the row `b` back, then inverts the row `b - a` back.
    FollowingFollowing { a: usize, b: usize },
    /// `a PRECEDING AND b PRECEDING`, `a >= b >= 1`: steps the row `b`
    /// back, returns the current row, then inverts the row `a` back.
    PrecedingPreceding { a: usize, b: usize },
    /// `a PRECEDING` (or CURRENT ROW, `a = 0`) `AND b FOLLOWING` (or CURRENT
    /// ROW, `b = 0`): steps, returns the row `b` back, then inverts the row
    /// `a` before that.
    Around { a: usize, b: usize },
    /// `... AND UNBOUNDED FOLLOWING`: every row steps first, then each row
    /// returns after the rows before its frame start were inverted. `start`
    /// is the start offset relative to the current row.
    ToUnbounded { start: i64 },
}

fn rows_schedule(frame: &ResolvedFrame) -> Option<RowsSchedule> {
    let offset = |n: i64| usize::try_from(n).ok();
    match (&frame.start, &frame.end) {
        (ResolvedBound::Following(a), ResolvedBound::Following(b)) if a <= b && *a >= 1 => {
            Some(RowsSchedule::FollowingFollowing {
                a: offset(*a)?,
                b: offset(*b)?,
            })
        }
        (ResolvedBound::Preceding(a), ResolvedBound::Preceding(b)) if a >= b && *b >= 1 => {
            Some(RowsSchedule::PrecedingPreceding {
                a: offset(*a)?,
                b: offset(*b)?,
            })
        }
        (ResolvedBound::Preceding(a), ResolvedBound::CurrentRow) => Some(RowsSchedule::Around {
            a: offset(*a)?,
            b: 0,
        }),
        (ResolvedBound::Preceding(a), ResolvedBound::Following(b)) => Some(RowsSchedule::Around {
            a: offset(*a)?,
            b: offset(*b)?,
        }),
        (ResolvedBound::CurrentRow, ResolvedBound::CurrentRow) => {
            Some(RowsSchedule::Around { a: 0, b: 0 })
        }
        (ResolvedBound::CurrentRow, ResolvedBound::Following(b)) => Some(RowsSchedule::Around {
            a: 0,
            b: offset(*b)?,
        }),
        (ResolvedBound::Preceding(a), ResolvedBound::UnboundedFollowing) => {
            Some(RowsSchedule::ToUnbounded { start: -*a })
        }
        (ResolvedBound::CurrentRow, ResolvedBound::UnboundedFollowing) => {
            Some(RowsSchedule::ToUnbounded { start: 0 })
        }
        (ResolvedBound::Following(a), ResolvedBound::UnboundedFollowing) => {
            Some(RowsSchedule::ToUnbounded { start: *a })
        }
        // Frames that are always empty, or start at UNBOUNDED PRECEDING.
        _ => None,
    }
}

/// One partition's accumulator and cursor positions.
struct Machine<'a> {
    kind: SumKind,
    acc: SumAcc,
    values: &'a [SqlValue],
    stepped: usize,
    inverted: usize,
    out: Vec<SqlValue>,
}

impl<'a> Machine<'a> {
    fn new(kind: SumKind, values: &'a [SqlValue]) -> Self {
        Self {
            kind,
            acc: SumAcc::new(),
            values,
            stepped: 0,
            inverted: 0,
            out: vec![SqlValue::Null; values.len()],
        }
    }

    /// Advance the end cursor over row `row` (rows arrive in order).
    fn step_to(&mut self, row: usize) {
        while self.stepped <= row && self.stepped < self.values.len() {
            self.acc.step_value(&self.values[self.stepped]);
            self.stepped += 1;
        }
    }

    /// Advance the start cursor so rows before `row` are removed.
    fn invert_before(&mut self, row: usize) {
        let limit = row.min(self.stepped);
        while self.inverted < limit {
            self.acc.inverse_value(&self.values[self.inverted]);
            self.inverted += 1;
        }
    }

    fn ret(&mut self, row: usize) -> Result<()> {
        self.out[row] = match self.kind {
            SumKind::Sum => self.acc.sum()?,
            SumKind::Total => self.acc.total(),
            SumKind::Avg => self.acc.avg(),
        };
        Ok(())
    }

    fn run_rows(mut self, schedule: RowsSchedule) -> Result<Vec<SqlValue>> {
        let n = self.values.len();
        match schedule {
            RowsSchedule::FollowingFollowing { a, b } => {
                let lag = b - a;
                for i in 0..n + b {
                    if i < n {
                        self.step_to(i);
                    }
                    if i >= b && i - b < n {
                        self.ret(i - b)?;
                    }
                    if i >= lag && i - lag < n {
                        self.invert_before(i - lag + 1);
                    }
                }
            }
            RowsSchedule::PrecedingPreceding { a, b } => {
                for i in 0..n {
                    if i >= b {
                        self.step_to(i - b);
                    }
                    self.ret(i)?;
                    if i >= a {
                        self.invert_before(i - a + 1);
                    }
                }
            }
            RowsSchedule::Around { a, b } => {
                for i in 0..n + b {
                    if i < n {
                        self.step_to(i);
                    }
                    if i >= b && i - b < n {
                        let k = i - b;
                        self.ret(k)?;
                        if k >= a {
                            self.invert_before(k - a + 1);
                        }
                    }
                }
            }
            RowsSchedule::ToUnbounded { start } => {
                if n > 0 {
                    self.step_to(n - 1);
                }
                for k in 0..n {
                    let first = (k as i64 + start).max(0) as usize;
                    self.invert_before(first);
                    self.ret(k)?;
                }
            }
        }
        Ok(self.out)
    }

    /// RANGE / GROUPS: remove the rows that left the frame, then add the
    /// rows that entered it. A row the frame skipped entirely is added
    /// before it is removed, as SQLite's end cursor passes it first.
    fn run_bounds(
        mut self,
        frame: &ResolvedFrame,
        layout: &CachedWindowPartition,
    ) -> Result<Vec<SqlValue>> {
        let n = self.values.len();
        for k in 0..n {
            let (start, end) = frame_bounds(frame, k, &layout.peer_ids, &layout.peer_ranges, n);
            if start <= end {
                if start > 0 {
                    self.step_to(start - 1);
                }
                self.invert_before(start);
                self.step_to(end);
            } else {
                self.invert_before(start);
            }
            self.ret(k)?;
        }
        Ok(self.out)
    }
}

pub(super) fn sliding_sum_window(
    func_name: &str,
    args: &[Expr],
    rows: &[SqlRow],
    layouts: &[CachedWindowPartition],
    frame: &ResolvedFrame,
    bindings: &[Option<SqlValue>],
    results: &mut [SqlValue],
) -> Result<bool> {
    let kind = match func_name {
        "sum" => SumKind::Sum,
        "total" => SumKind::Total,
        // The Postgres dialect answers integer avg() as numeric text; that
        // stays on the per-frame accumulator.
        "avg" if !crate::value::postgres_result_dialect() => SumKind::Avg,
        _ => return Ok(false),
    };
    if frame.exclude != ExcludeMode::NoOthers
        || matches!(frame.start, ResolvedBound::UnboundedPreceding)
    {
        return Ok(false);
    }
    let schedule = match frame.units {
        WindowFrameUnits::Rows => match rows_schedule(frame) {
            Some(schedule) => Some(schedule),
            None => return Ok(false),
        },
        WindowFrameUnits::Range | WindowFrameUnits::Groups => None,
    };
    let Some(arg) = args.first() else {
        return Ok(false);
    };
    for layout in layouts {
        let values = layout
            .order_index_map
            .iter()
            .map(|row_idx| eval_scalar(arg, &rows[*row_idx].context(), bindings))
            .collect::<Result<Vec<SqlValue>>>()?;
        let machine = Machine::new(kind, &values);
        let out = match schedule {
            Some(schedule) => machine.run_rows(schedule)?,
            None => machine.run_bounds(frame, layout)?,
        };
        for (value, row_idx) in out.into_iter().zip(&layout.order_index_map) {
            results[*row_idx] = value;
        }
    }
    Ok(true)
}
