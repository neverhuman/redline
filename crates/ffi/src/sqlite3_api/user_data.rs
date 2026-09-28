//! Ownership of the `user_data` pointer a caller hands to
//! `sqlite3_create_function_v2` or `sqlite3_create_collation_v2`.
//!
//! SQLite calls the registrar's destructor once: when the function or
//! collation is replaced or deleted, or when its connection closes. A registry
//! entry holds an `Arc<UserData>` and every dispatch holds a clone for the
//! length of the C callback, so the destructor runs after the last callback
//! still using the pointer returns. Registries drop their references only
//! after releasing their lock, because the destructor is caller code and may
//! call back into the library.

use std::ffi::c_void;

use super::udf::DestructorFn;

pub(crate) struct UserData {
    /// Held as `usize` so the value is `Send + Sync`; the registrar owns what
    /// it points at.
    addr: usize,
    destructor: Option<DestructorFn>,
}

impl UserData {
    pub(crate) fn new(ptr: *mut c_void, destructor: Option<DestructorFn>) -> Self {
        Self {
            addr: ptr as usize,
            destructor,
        }
    }

    pub(crate) fn as_ptr(&self) -> *mut c_void {
        self.addr as *mut c_void
    }
}

impl Drop for UserData {
    fn drop(&mut self) {
        if let Some(destroy) = self.destructor {
            // The last owner is dropping, so no callback holds the pointer.
            // SAFETY: `destroy` and the pointer are the pair a caller passed
            // to a `_v2` registration, which makes `destroy(user_data)` sound
            // to call once; Drop runs once.
            unsafe { destroy(self.as_ptr()) };
        }
    }
}
