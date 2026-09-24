//! Tuple-to-morsel adapter. Lives at the ingress edge of the vector path
//! and converts row-at-a-time producers into columnar batches.

use bumpalo::Bump;
use redlinedb_kernel::catalog::ValueRef;
use smallvec::SmallVec;

use super::arena::BytesArena;
use super::bitmap::Bitmap;
use super::column::ColumnBatch;
use super::{MAX_BATCH_ROWS, Morsel};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ColumnKind {
    I64,
    F64,
    Text,
    Blob,
    Null,
}

enum BuilderColumn<'a> {
    I64(SmallVec<[i64; 256]>),
    F64(SmallVec<[f64; 256]>),
    Text(BytesArena<'a>),
    Blob(BytesArena<'a>),
    Null(usize),
}

pub struct MorselBuilder<'a> {
    columns: SmallVec<[BuilderColumn<'a>; 8]>,
    validity: Bitmap,
    len: usize,
    cap: usize,
}

impl<'a> MorselBuilder<'a> {
    pub fn with_capacity(arena: &'a Bump, kinds: &[ColumnKind], cap_rows: usize) -> Self {
        let cap = cap_rows.min(MAX_BATCH_ROWS);
        let mut columns: SmallVec<[BuilderColumn<'a>; 8]> = SmallVec::with_capacity(kinds.len());
        for k in kinds {
            columns.push(match k {
                ColumnKind::I64 => BuilderColumn::I64(SmallVec::with_capacity(cap)),
                ColumnKind::F64 => BuilderColumn::F64(SmallVec::with_capacity(cap)),
                ColumnKind::Text => BuilderColumn::Text(BytesArena::new(arena, cap)),
                ColumnKind::Blob => BuilderColumn::Blob(BytesArena::new(arena, cap)),
                ColumnKind::Null => BuilderColumn::Null(0),
            });
        }
        Self {
            columns,
            validity: Bitmap::new(cap),
            len: 0,
            cap,
        }
    }

    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }

    pub fn push_row(&mut self, values: &[ValueRef<'_>]) -> Result<(), &'static str> {
        if self.len >= self.cap {
            return Err("morsel at capacity");
        }
        if values.len() != self.columns.len() {
            return Err("row width mismatch");
        }
        // Check every column before appending. A mismatch on a later
        // column must not leave the earlier columns one row longer than
        // `len`, or `finish` would ship a torn morsel.
        for (col, v) in self.columns.iter().zip(values.iter()) {
            if !column_accepts(col, v) {
                return Err("row/column kind mismatch");
            }
        }
        for (col, v) in self.columns.iter_mut().zip(values.iter()) {
            match (col, v) {
                (BuilderColumn::I64(buf), ValueRef::Integer(x)) => buf.push(*x),
                (BuilderColumn::F64(buf), ValueRef::Real(x)) => buf.push(*x),
                (BuilderColumn::Text(arena), ValueRef::Text(s)) => {
                    arena.push(s.as_bytes());
                }
                (BuilderColumn::Blob(arena), ValueRef::Blob(b)) => {
                    arena.push(b);
                }
                (BuilderColumn::Null(n), ValueRef::Null) => {
                    *n += 1;
                }
                _ => unreachable!("push_row preflight accepted this row"),
            }
        }
        self.validity.set(self.len);
        self.len += 1;
        Ok(())
    }

    pub fn invalidate(&mut self, row: usize) {
        if row < self.len {
            self.validity.clear(row);
        }
    }

    pub fn finish(self) -> Morsel<'a> {
        let len = self.len;
        let mut columns: SmallVec<[ColumnBatch<'a>; 8]> =
            SmallVec::with_capacity(self.columns.len());
        for col in self.columns {
            columns.push(match col {
                BuilderColumn::I64(v) => ColumnBatch::I64(v),
                BuilderColumn::F64(v) => ColumnBatch::F64(v),
                BuilderColumn::Text(a) => ColumnBatch::Text(a),
                BuilderColumn::Blob(a) => ColumnBatch::Blob(a),
                BuilderColumn::Null(n) => ColumnBatch::Null { len: n },
            });
        }
        debug_assert!(len <= MAX_BATCH_ROWS, "morsel exceeds MAX_BATCH_ROWS");
        for c in &columns {
            debug_assert_eq!(c.len(), len, "column len mismatch in finish()");
        }
        Morsel {
            columns,
            validity: self.validity,
            len: len as u16,
        }
    }
}

fn column_accepts(col: &BuilderColumn<'_>, value: &ValueRef<'_>) -> bool {
    matches!(
        (col, value),
        (BuilderColumn::I64(_), ValueRef::Integer(_))
            | (BuilderColumn::F64(_), ValueRef::Real(_))
            | (BuilderColumn::Text(_), ValueRef::Text(_))
            | (BuilderColumn::Blob(_), ValueRef::Blob(_))
            | (BuilderColumn::Null(_), ValueRef::Null)
    )
}

#[cfg(test)]
mod tests {
    use bumpalo::Bump;
    use redlinedb_kernel::catalog::ValueRef;

    use super::*;

    #[test]
    fn mismatch_does_not_grow_earlier_columns() {
        let bump = Bump::new();
        let kinds = [ColumnKind::I64, ColumnKind::Text];
        let mut builder = MorselBuilder::with_capacity(&bump, &kinds, 4);
        builder
            .push_row(&[ValueRef::Integer(1), ValueRef::Text("a")])
            .unwrap();
        assert!(
            builder
                .push_row(&[ValueRef::Integer(2), ValueRef::Integer(3)])
                .is_err()
        );
        assert!(
            builder
                .push_row(&[ValueRef::Integer(4), ValueRef::Null])
                .is_err()
        );
        builder
            .push_row(&[ValueRef::Integer(5), ValueRef::Text("b")])
            .unwrap();
        let morsel = builder.finish();
        assert_eq!(morsel.len(), 2);
        assert_eq!(morsel.columns[0].len(), 2);
        assert_eq!(morsel.columns[1].len(), 2);
        assert!(matches!(
            morsel.columns[0].get(0),
            Some(ValueRef::Integer(1))
        ));
        assert!(matches!(
            morsel.columns[0].get(1),
            Some(ValueRef::Integer(5))
        ));
        assert!(matches!(
            morsel.columns[1].get(1),
            Some(ValueRef::Text("b"))
        ));
    }
}
