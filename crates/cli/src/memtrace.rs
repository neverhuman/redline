//! `-memtrace`: report every heap allocation on stderr, the way sqlite3's
//! `-memtrace` reports its own: `MEMTRACE: allocate 40 bytes`,
//! `MEMTRACE: free 40 bytes`, `MEMTRACE: resize 16 -> 32 bytes`.
//!
//! The lines describe RedlineDB's allocations, made by Rust's global
//! allocator, so their count and sizes are RedlineDB's, not SQLite's. The
//! wrapper costs one relaxed atomic load per allocation until the option
//! turns tracing on.

use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Set while this thread writes a trace line, so an allocation made by
    /// the write itself is not traced again.
    static WRITING: Cell<bool> = const { Cell::new(false) };
}

/// Start tracing allocations (`-memtrace`).
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// The process's global allocator `A`, tracing each call once `-memtrace`
/// turned tracing on.
pub struct Traced<A>(pub A);

// SAFETY: every method forwards to the wrapped allocator with the caller's
// arguments unchanged and returns its result unchanged, so `Traced<A>` keeps
// each `GlobalAlloc` contract `A` keeps. Tracing only reads the sizes and
// writes to stderr from a stack buffer; it allocates nothing itself.
unsafe impl<A: GlobalAlloc> GlobalAlloc for Traced<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `alloc`'s contract for `layout`.
        let ptr = unsafe { self.0.alloc(layout) };
        if !ptr.is_null() {
            trace(b"allocate ", layout.size(), None);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `alloc_zeroed`'s contract for `layout`.
        let ptr = unsafe { self.0.alloc_zeroed(layout) };
        if !ptr.is_null() {
            trace(b"allocate ", layout.size(), None);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        trace(b"free ", layout.size(), None);
        // SAFETY: the caller guarantees `ptr` came from this allocator with
        // `layout`, which is what the wrapped allocator requires.
        unsafe { self.0.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller upholds `realloc`'s contract for `ptr`,
        // `layout` and `new_size`, which the wrapped allocator requires.
        let new_ptr = unsafe { self.0.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            trace(b"resize ", layout.size(), Some(new_size));
        }
        new_ptr
    }
}

/// Write `MEMTRACE: <what><size>[ -> <new_size>] bytes` to stderr without
/// allocating.
fn trace(what: &[u8], size: usize, new_size: Option<usize>) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let _ = WRITING.try_with(|writing| {
        if writing.replace(true) {
            return;
        }
        let mut line = [0u8; 96];
        let mut len = 0;
        let mut push = |bytes: &[u8]| {
            let end = (len + bytes.len()).min(line.len());
            line[len..end].copy_from_slice(&bytes[..end - len]);
            len = end;
        };
        let mut number = itoa::Buffer::new();
        push(b"MEMTRACE: ");
        push(what);
        push(number.format(size).as_bytes());
        if let Some(new_size) = new_size {
            push(b" -> ");
            push(number.format(new_size).as_bytes());
        }
        push(b" bytes\n");
        let _ = std::io::stderr().write_all(&line[..len]);
        writing.set(false);
    });
}
