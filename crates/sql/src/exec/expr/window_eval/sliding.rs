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
//! ROWS frames follow the operation order of SQLite's `windowCodeStep`,
//! and GROUPS frames the same schedules over peer groups. RANGE frames use
//! the same accumulator in `windowCodeStep`'s RANGE order (see
//! `run_bounds`).

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
    /// The units a ROWS-style schedule counts: each row for ROWS, each peer
    /// group (first row, last row) for GROUPS, which SQLite steps, returns
    /// and removes a whole group at a time.
    groups: Option<&'a [(usize, usize)]>,
    stepped: usize,
    inverted: usize,
    out: Vec<SqlValue>,
}

impl<'a> Machine<'a> {
    fn new(kind: SumKind, values: &'a [SqlValue], groups: Option<&'a [(usize, usize)]>) -> Self {
        Self {
            kind,
            acc: SumAcc::new(),
            values,
            groups,
            stepped: 0,
            inverted: 0,
            out: vec![SqlValue::Null; values.len()],
        }
    }

    fn unit_count(&self) -> usize {
        self.groups.map_or(self.values.len(), <[_]>::len)
    }

    /// The first row of unit `unit`, or the row count past the last unit.
    fn unit_first_row(&self, unit: usize) -> usize {
        match self.groups {
            None => unit.min(self.values.len()),
            Some(groups) => groups.get(unit).map_or(self.values.len(), |g| g.0),
        }
    }

    fn step_unit(&mut self, unit: usize) {
        let last = match self.groups {
            None => unit,
            Some(groups) => groups.get(unit).map_or(self.values.len(), |g| g.1),
        };
        self.step_to(last);
    }

    fn invert_before_unit(&mut self, unit: usize) {
        self.invert_before(self.unit_first_row(unit));
    }

    fn ret_unit(&mut self, unit: usize) -> Result<()> {
        let first = self.unit_first_row(unit);
        let end = self.unit_first_row(unit + 1);
        for row in first..end.max(first + 1).min(self.values.len()) {
            self.ret(row)?;
        }
        Ok(())
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
        let n = self.unit_count();
        match schedule {
            RowsSchedule::FollowingFollowing { a, b } => {
                // Iteration `i` steps row `i`, returns row `i - b` and
                // inverts the rows before `i - lag + 1`. Past the last row
                // only the inverts go on until the first return, and a run
                // of inverts ends where its last one does, so the idle
                // stretch collapses to one call: an offset near 2^63 costs
                // no more than one at `n`.
                let lag = b - a;
                let at = |this: &mut Self, i: usize| -> Result<()> {
                    if i < n {
                        this.step_unit(i);
                    }
                    if i >= b && i - b < n {
                        this.ret_unit(i - b)?;
                    }
                    if i >= lag && i - lag < n {
                        this.invert_before_unit(i - lag + 1);
                    }
                    Ok(())
                };
                for i in 0..n {
                    at(&mut self, i)?;
                }
                let first_return = n.max(b);
                if first_return > n && first_return - 1 >= lag {
                    self.invert_before_unit(first_return - lag);
                }
                for i in first_return..n.saturating_add(b) {
                    at(&mut self, i)?;
                }
            }
            RowsSchedule::PrecedingPreceding { a, b } => {
                for i in 0..n {
                    if i >= b {
                        self.step_unit(i - b);
                    }
                    self.ret_unit(i)?;
                    if i >= a {
                        self.invert_before_unit(i - a + 1);
                    }
                }
            }
            RowsSchedule::Around { a, b } => {
                // Past the last row nothing steps, so an offset beyond the
                // partition acts as one at its end.
                let (a, b) = (a.min(n), b.min(n));
                for i in 0..n + b {
                    if i < n {
                        self.step_unit(i);
                    }
                    if i >= b && i - b < n {
                        let k = i - b;
                        self.ret_unit(k)?;
                        if k >= a {
                            self.invert_before_unit(k - a + 1);
                        }
                    }
                }
            }
            RowsSchedule::ToUnbounded { start } => {
                if n > 0 {
                    self.step_unit(n - 1);
                }
                for k in 0..n {
                    let first = (k as i64).saturating_add(start).max(0) as usize;
                    self.invert_before_unit(first);
                    self.ret_unit(k)?;
                }
            }
        }
        Ok(self.out)
    }

    /// RANGE / GROUPS. Most frames remove the rows that left the frame,
    /// then add the rows that entered it; a row the frame skipped entirely
    /// is added before it is removed, as SQLite's end cursor passes it
    /// first. A RANGE frame whose bounds are both `n PRECEDING` or both
    /// `n FOLLOWING` adds first: `windowCodeStep` moves its end cursor
    /// before its start cursor there, which decides when an INTEGER
    /// overflow shows.
    fn run_bounds(
        mut self,
        frame: &ResolvedFrame,
        layout: &CachedWindowPartition,
    ) -> Result<Vec<SqlValue>> {
        let n = self.values.len();
        let add_first = matches!(frame.units, WindowFrameUnits::Range)
            && matches!(
                (&frame.start, &frame.end),
                (ResolvedBound::Preceding(_), ResolvedBound::Preceding(_))
                    | (ResolvedBound::Following(_), ResolvedBound::Following(_))
            );
        for k in 0..n {
            let (start, end) = frame_bounds(frame, k, &layout.peer_ids, &layout.peer_ranges, n);
            if add_first {
                // An empty frame at the head (`n PRECEDING` before the
                // first row) has passed no row yet; one at the tail
                // (`n FOLLOWING` past the last) has passed them all.
                if start <= end {
                    self.step_to(end);
                } else if matches!(frame.end, ResolvedBound::Following(_)) && n > 0 {
                    self.step_to(n - 1);
                }
                self.invert_before(start);
            } else if start <= end {
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
    // GROUPS frames run the ROWS schedules over peer groups, as
    // `windowCodeStep` does; RANGE frames follow their own order.
    let schedule = match frame.units {
        WindowFrameUnits::Rows | WindowFrameUnits::Groups => match rows_schedule(frame) {
            Some(schedule) => Some(schedule),
            None => return Ok(false),
        },
        WindowFrameUnits::Range => None,
    };
    let by_group = matches!(frame.units, WindowFrameUnits::Groups);
    let Some(arg) = args.first() else {
        return Ok(false);
    };
    for layout in layouts {
        let values = layout
            .order_index_map
            .iter()
            .map(|row_idx| eval_scalar(arg, &rows[*row_idx].context(), bindings))
            .collect::<Result<Vec<SqlValue>>>()?;
        let groups = by_group.then_some(layout.peer_ranges.as_slice());
        let machine = Machine::new(kind, &values, groups);
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
