//! S8-10: `rldb_open_v2` validates its `rldb_config` before using it.
//!
//! `struct_size` is read first and bounds every later read, so a caller built
//! against a shorter (older) struct is never read past its end: the short
//! case places the struct against a `PROT_NONE` guard page, where reading the
//! whole current struct faults instead of failing an assert. A size below 4 or
//! above 4096, a size that ends inside a field, a non-zero `flags`, an unknown
//! `durability`, or a non-zero byte past the fields this library knows is
//! `RLDB_MISUSE` with no handle and nothing created. A zero field keeps the
//! built-in default instead of turning the limit off.

use std::ffi::CString;
use std::mem::size_of;
use std::os::raw::{c_char, c_int};
use std::path::{Path, PathBuf};
use std::ptr;

use redlinedb::{
    rldb, rldb_backup, rldb_backup_init, rldb_close, rldb_column_int64, rldb_config, rldb_exec,
    rldb_finalize, rldb_open, rldb_open_v2, rldb_prepare_v2, rldb_step, rldb_stmt,
};

const RLDB_OK: c_int = 0;
const RLDB_MISUSE: c_int = 21;
const RLDB_ROW: c_int = 100;

const RLDB_DURABILITY_DEFAULT: u32 = 0;
const RLDB_DURABILITY_STRICT: u32 = 1;
const RLDB_DURABILITY_NORMAL: u32 = 2;

/// Two anonymous pages; the second is `PROT_NONE`. Unmapped on drop.
struct GuardPage {
    base: *mut u8,
    page: usize,
}

impl GuardPage {
    fn new() -> Self {
        // SAFETY: sysconf has no memory preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        assert!(page > 0, "page size");
        let page = page as usize;
        // SAFETY: anonymous private mapping with no address hint; the result
        // is checked against MAP_FAILED before use.
        let base = unsafe {
            libc::mmap(
                ptr::null_mut(),
                page * 2,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        assert_ne!(base, libc::MAP_FAILED, "mmap");
        // SAFETY: `base + page` is the second page of the mapping just made.
        let rc =
            unsafe { libc::mprotect((base as *mut u8).add(page).cast(), page, libc::PROT_NONE) };
        assert_eq!(rc, 0, "mprotect");
        Self {
            base: base.cast(),
            page,
        }
    }

    /// Copy `bytes` so they end exactly at the guard page; return their start.
    fn place_at_edge(&self, bytes: &[u8]) -> *const rldb_config {
        assert!(bytes.len() <= self.page);
        // SAFETY: the destination lies in the readable first page and ends at
        // its last byte.
        unsafe {
            let start = self.base.add(self.page - bytes.len());
            ptr::copy_nonoverlapping(bytes.as_ptr(), start, bytes.len());
            start as *const rldb_config
        }
    }
}

impl Drop for GuardPage {
    fn drop(&mut self) {
        // SAFETY: unmaps exactly the mapping created in `new`.
        unsafe { libc::munmap(self.base.cast(), self.page * 2) };
    }
}

fn full_config() -> rldb_config {
    rldb_config {
        struct_size: size_of::<rldb_config>() as u32,
        flags: 0,
        durability: RLDB_DURABILITY_DEFAULT,
        cache_bytes: 0,
        work_mem_bytes: 0,
        max_spill_bytes: 0,
        statement_cache_capacity: 0,
        busy_timeout_ms: 0,
    }
}

fn db_path(dir: &Path) -> PathBuf {
    dir.join("config.redline")
}

fn open_with(path: &Path, config: *const rldb_config) -> (c_int, *mut rldb) {
    let c_path = CString::new(path.to_str().expect("utf8")).expect("cstring");
    let mut db = ptr::dangling_mut::<rldb>();
    // SAFETY: `c_path` is NUL-terminated, `db` is a writable slot, and each
    // caller passes NULL or a config readable for its `struct_size` bytes.
    let rc = unsafe { rldb_open_v2(c_path.as_ptr(), config, &mut db) };
    if rc != RLDB_OK {
        assert!(
            db.is_null() || db == ptr::dangling_mut::<rldb>(),
            "a failed open must not hand out a handle"
        );
        return (rc, ptr::null_mut());
    }
    assert!(!db.is_null());
    (rc, db)
}

fn assert_refused(config: *const rldb_config) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = db_path(dir.path());
    let (rc, _) = open_with(&path, config);
    assert_eq!(rc, RLDB_MISUSE, "config must be refused");
    assert!(
        !path.exists(),
        "a refused open must not create the database"
    );
}

fn exec(db: *mut rldb, sql: &str) {
    let c_sql = CString::new(sql).expect("cstring");
    // SAFETY: `db` is a live handle and `c_sql` is NUL-terminated; no
    // callback and no error-message slot.
    let rc = unsafe { rldb_exec(db, c_sql.as_ptr(), None, ptr::null_mut(), ptr::null_mut()) };
    assert_eq!(rc, RLDB_OK, "exec {sql}");
}

/// Step `sql` once and return its first column, or the failing step code.
fn query_i64(db: *mut rldb, sql: &str) -> Result<i64, c_int> {
    let c_sql = CString::new(sql).expect("cstring");
    let mut stmt: *mut rldb_stmt = ptr::null_mut();
    let mut tail: *const c_char = ptr::null();
    // SAFETY: `db` is live, `c_sql` is NUL-terminated (nbytes -1), and both
    // out-pointers are writable.
    let rc = unsafe { rldb_prepare_v2(db, c_sql.as_ptr(), -1, &mut stmt, &mut tail) };
    assert_eq!(rc, RLDB_OK, "prepare {sql}");
    // SAFETY: `stmt` came from a successful prepare and is finalized below.
    let step = unsafe { rldb_step(stmt) };
    let value = if step == RLDB_ROW {
        // SAFETY: `stmt` is live and positioned on a row.
        Ok(unsafe { rldb_column_int64(stmt, 0) })
    } else {
        Err(step)
    };
    // SAFETY: finalizes the statement prepared above exactly once.
    unsafe { rldb_finalize(stmt) };
    value
}

fn close(db: *mut rldb) {
    // SAFETY: `db` is a live handle opened by this test and closed once.
    assert_eq!(unsafe { rldb_close(db) }, RLDB_OK);
}

/// A sort over enough rows to need working memory: it fails when the query
/// memory budget and the spill budget are both zero.
fn sort_needing_work_mem(db: *mut rldb) -> Result<i64, c_int> {
    exec(db, "CREATE TABLE t(a INTEGER, b TEXT)");
    exec(
        db,
        "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 2000) \
         INSERT INTO t SELECT x, printf('row-%06d', x) FROM c",
    );
    query_i64(db, "SELECT a FROM t ORDER BY b DESC LIMIT 1")
}

#[test]
fn open_v2_rejects_zero_struct_size() {
    let config = rldb_config {
        struct_size: 0,
        ..full_config()
    };
    assert_refused(&config);
}

#[test]
fn open_v2_rejects_out_of_range_struct_size() {
    for size in [1, 3, 4097, u32::MAX] {
        let config = rldb_config {
            struct_size: size,
            ..full_config()
        };
        assert_refused(&config);
    }
}

#[test]
fn open_v2_rejects_struct_size_inside_a_field() {
    // 20 ends halfway through `cache_bytes` (offset 16, 8 bytes).
    let config = rldb_config {
        struct_size: 20,
        ..full_config()
    };
    assert_refused(&config);
}

#[test]
fn open_v2_rejects_unknown_flags() {
    for flags in [1, 0x8000_0000] {
        let config = rldb_config {
            flags,
            ..full_config()
        };
        assert_refused(&config);
    }
}

#[test]
fn open_v2_rejects_unknown_durability() {
    for durability in [3, u32::MAX] {
        let config = rldb_config {
            durability,
            ..full_config()
        };
        assert_refused(&config);
    }
}

#[test]
fn open_v2_rejects_non_zero_bytes_past_known_fields() {
    // A caller built against a newer, larger struct that sets a field this
    // library does not know asks for something it cannot provide.
    let mut bytes = [0u8; 64];
    bytes[..4].copy_from_slice(&64u32.to_ne_bytes());
    bytes[56] = 1;
    assert_refused(bytes.as_ptr().cast());

    // The same larger struct with the unknown tail zeroed opens.
    bytes[56] = 0;
    let dir = tempfile::tempdir().expect("tempdir");
    let (rc, db) = open_with(&db_path(dir.path()), bytes.as_ptr().cast());
    assert_eq!(rc, RLDB_OK);
    close(db);
}

#[test]
fn open_v2_short_struct_uses_defaults() {
    // A 12-byte struct (struct_size, flags, durability) ending at a guard
    // page: every field past byte 12 must be neither read nor taken from
    // the caller.
    let mut bytes = [0u8; 12];
    bytes[..4].copy_from_slice(&12u32.to_ne_bytes());
    bytes[8..].copy_from_slice(&RLDB_DURABILITY_NORMAL.to_ne_bytes());
    let guard = GuardPage::new();
    let config = guard.place_at_edge(&bytes);
    let dir = tempfile::tempdir().expect("tempdir");
    let (rc, db) = open_with(&db_path(dir.path()), config);
    assert_eq!(rc, RLDB_OK);
    assert_eq!(
        query_i64(db, "PRAGMA synchronous"),
        Ok(1),
        "durability read"
    );
    assert_eq!(sort_needing_work_mem(db), Ok(2000), "default query memory");
    close(db);
}

#[test]
fn open_v2_minimal_struct_opens() {
    let bytes = 4u32.to_ne_bytes();
    let guard = GuardPage::new();
    let config = guard.place_at_edge(&bytes);
    let dir = tempfile::tempdir().expect("tempdir");
    let (rc, db) = open_with(&db_path(dir.path()), config);
    assert_eq!(rc, RLDB_OK);
    assert_eq!(query_i64(db, "PRAGMA synchronous"), Ok(2));
    close(db);
}

#[test]
fn open_v2_zero_fields_keep_defaults() {
    // Before validation, zero cache/work_mem/spill/statement-cache/busy
    // fields replaced the defaults, so this sort failed its memory budget.
    let config = full_config();
    let dir = tempfile::tempdir().expect("tempdir");
    let (rc, db) = open_with(&db_path(dir.path()), &config);
    assert_eq!(rc, RLDB_OK);
    assert_eq!(
        query_i64(db, "PRAGMA synchronous"),
        Ok(2),
        "default is FULL"
    );
    assert_eq!(sort_needing_work_mem(db), Ok(2000));
    close(db);
}

#[test]
fn open_v2_durability_normal_applied() {
    let config = rldb_config {
        durability: RLDB_DURABILITY_NORMAL,
        ..full_config()
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let (rc, db) = open_with(&db_path(dir.path()), &config);
    assert_eq!(rc, RLDB_OK);
    assert_eq!(query_i64(db, "PRAGMA synchronous"), Ok(1), "NORMAL");
    close(db);
}

#[test]
fn open_v2_durability_strict_applied() {
    let config = rldb_config {
        durability: RLDB_DURABILITY_STRICT,
        ..full_config()
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let (rc, db) = open_with(&db_path(dir.path()), &config);
    assert_eq!(rc, RLDB_OK);
    assert_eq!(query_i64(db, "PRAGMA synchronous"), Ok(2), "FULL");
    close(db);
}

#[test]
fn backup_init_rejects_a_destination_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src_path = CString::new(db_path(dir.path()).to_str().expect("utf8")).expect("cstring");
    let mut src: *mut rldb = ptr::null_mut();
    // SAFETY: NUL-terminated path and a writable handle slot.
    assert_eq!(unsafe { rldb_open(src_path.as_ptr(), &mut src) }, RLDB_OK);
    let dst = dir.path().join("copy.redline");
    let c_dst = CString::new(dst.to_str().expect("utf8")).expect("cstring");
    let config = full_config();
    let mut backup = ptr::dangling_mut::<rldb_backup>();
    // SAFETY: `src` is live, `c_dst` is NUL-terminated, `config` is a whole
    // struct and `backup` is a writable slot.
    let rc = unsafe { rldb_backup_init(src, c_dst.as_ptr(), &config, &mut backup) };
    assert_eq!(rc, RLDB_MISUSE, "a destination config is never applied");
    assert!(
        backup.is_null() || backup == ptr::dangling_mut::<rldb_backup>(),
        "no backup handle on failure"
    );
    assert!(!dst.exists());
    close(src);
}

#[test]
fn header_durability_values_match() {
    let header =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/c-abi/redlinedb.h");
    let text = std::fs::read_to_string(&header).expect("read redlinedb.h");
    let value = |name: &str| -> u32 {
        let prefix = format!("#define {name} ");
        let line = text
            .lines()
            .find(|line| line.starts_with(&prefix))
            .unwrap_or_else(|| panic!("redlinedb.h does not define {name}"));
        line[prefix.len()..]
            .split_whitespace()
            .next()
            .and_then(|token| token.parse().ok())
            .unwrap_or_else(|| panic!("{name} is not a number: {line}"))
    };
    assert_eq!(value("RLDB_DURABILITY_DEFAULT"), RLDB_DURABILITY_DEFAULT);
    assert_eq!(value("RLDB_DURABILITY_STRICT"), RLDB_DURABILITY_STRICT);
    assert_eq!(value("RLDB_DURABILITY_NORMAL"), RLDB_DURABILITY_NORMAL);
}
