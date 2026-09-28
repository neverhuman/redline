//! The RedlineDB v4.x index-key format (index-format epoch 2), kept only so
//! tests can write a database exactly as a v4 build would have and then
//! prove that opening it rebuilds every index at the current epoch.
//!
//! Epoch 2 gave INTEGER (tag 0x10) and REAL (tag 0x20) separate key spaces,
//! so an index ordered every INTEGER before every REAL and `x = 2` could not
//! find a stored `2.0`. Nothing in a production build writes this format.

use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::value::ValueRef;

thread_local! {
    static V4_INDEX_FORMAT: Cell<bool> = const { Cell::new(false) };
}

/// How many threads are inside [`with_v4_index_format_for_tests`]. The hot
/// paths read this relaxed counter first so the thread-local is consulted
/// only while a test holds the hook.
static V4_INDEX_FORMAT_ARMED: AtomicUsize = AtomicUsize::new(0);

/// Run `f` with this thread writing index keys and B-tree pages in the
/// RedlineDB v4.x format (index-format epoch 2). Test-only: a database
/// written inside `f` is what a v4 build would have left on disk, including
/// INTEGER/REAL keys a v5 build treats as equal.
#[doc(hidden)]
pub fn with_v4_index_format_for_tests<R>(f: impl FnOnce() -> R) -> R {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            V4_INDEX_FORMAT.with(|flag| flag.set(self.0));
            V4_INDEX_FORMAT_ARMED.fetch_sub(1, Ordering::SeqCst);
        }
    }
    V4_INDEX_FORMAT_ARMED.fetch_add(1, Ordering::SeqCst);
    let _reset = Reset(V4_INDEX_FORMAT.with(|flag| flag.replace(true)));
    f()
}

/// True while the calling thread is inside
/// [`with_v4_index_format_for_tests`].
#[inline]
pub(crate) fn v4_index_format_active() -> bool {
    V4_INDEX_FORMAT_ARMED.load(Ordering::Relaxed) != 0 && V4_INDEX_FORMAT.with(Cell::get)
}

/// The epoch-2 part encoding, byte for byte as RedlineDB v4.1.0 wrote it.
pub(super) fn encode_part_v4(value: ValueRef<'_>, out: &mut Vec<u8>) {
    match value {
        ValueRef::Null => out.push(0x00),
        ValueRef::Integer(v) => {
            out.push(0x10);
            let sortable = (v as u64) ^ 0x8000_0000_0000_0000;
            out.extend_from_slice(&sortable.to_be_bytes());
        }
        ValueRef::Real(v) => {
            out.push(0x20);
            let bits = if v == 0.0 { 0.0 } else { v }.to_bits();
            let sortable = if bits & 0x8000_0000_0000_0000 != 0 {
                !bits
            } else {
                bits ^ 0x8000_0000_0000_0000
            };
            out.extend_from_slice(&sortable.to_be_bytes());
        }
        ValueRef::Text(v) => {
            out.push(0x30);
            escape_v4(v.as_bytes(), out);
        }
        ValueRef::Blob(v) => {
            out.push(0x40);
            escape_v4(v, out);
        }
    }
}

fn escape_v4(bytes: &[u8], out: &mut Vec<u8>) {
    for &byte in bytes {
        if byte == 0 {
            out.push(0);
            out.push(0xff);
        } else {
            out.push(byte);
        }
    }
    out.push(0);
    out.push(0);
}
