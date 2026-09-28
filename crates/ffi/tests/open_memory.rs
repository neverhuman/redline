//! `sqlite3_open(":memory:")` must open a private, ephemeral database and
//! must not create anything named `:memory:` in the working directory.
//!
//! This binary holds a single test because it changes the process working
//! directory; keeping it alone avoids racing other tests on that global.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;

use redlinedb::sqlite3_api::{
    sqlite3_backup_init, sqlite3_close, sqlite3_column_int64, sqlite3_exec, sqlite3_finalize,
    sqlite3_open, sqlite3_open_v2, sqlite3_prepare_v2, sqlite3_step,
};
use redlinedb::types::{sqlite3, sqlite3_stmt};
use redlinedb::{rldb_close, rldb_open};

unsafe extern "C" {
    // Exported by the library but not re-exported to Rust callers; the
    // handle is opaque to C, so it is declared as void*.
    fn sqlite3_db_filename(db: *mut c_void, name: *const c_char) -> *const c_char;
}

const SQLITE_OK: c_int = 0;
const SQLITE_CANTOPEN: c_int = 14;
const SQLITE_ROW: c_int = 100;
const SQLITE_OPEN_READWRITE: c_int = 0x02;
const SQLITE_OPEN_CREATE: c_int = 0x04;
const SQLITE_OPEN_URI: c_int = 0x40;
const SQLITE_OPEN_MEMORY: c_int = 0x80;

fn exec(db: *mut sqlite3, sql: &str) -> c_int {
    let sql = CString::new(sql).expect("cstring");
    unsafe { sqlite3_exec(db, sql.as_ptr(), None, ptr::null_mut(), ptr::null_mut()) }
}

fn count(db: *mut sqlite3, sql: &str) -> Option<i64> {
    let sql = CString::new(sql).expect("cstring");
    let mut stmt: *mut sqlite3_stmt = ptr::null_mut();
    if unsafe { sqlite3_prepare_v2(db, sql.as_ptr(), -1, &mut stmt, ptr::null_mut()) } != SQLITE_OK
    {
        return None;
    }
    let value = (unsafe { sqlite3_step(stmt) } == SQLITE_ROW)
        .then(|| unsafe { sqlite3_column_int64(stmt, 0) });
    assert_eq!(unsafe { sqlite3_finalize(stmt) }, SQLITE_OK);
    value
}

fn filename(db: *mut sqlite3) -> String {
    // SAFETY: db is an open connection and the name is NUL-terminated.
    let name = unsafe { sqlite3_db_filename(db.cast(), c"main".as_ptr()) };
    assert!(!name.is_null(), "main database has a filename");
    // SAFETY: non-null NUL-terminated string owned by the connection.
    unsafe { CStr::from_ptr(name) }
        .to_str()
        .expect("utf8")
        .to_owned()
}

fn entries(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read_dir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

#[test]
fn sqlite3_open_memory_creates_no_file() {
    let cwd = tempfile::tempdir().expect("tempdir");
    std::env::set_current_dir(cwd.path()).expect("chdir");
    let memory = c":memory:";

    // Two :memory: opens are two independent, working databases.
    let mut a: *mut sqlite3 = ptr::null_mut();
    let mut b: *mut sqlite3 = ptr::null_mut();
    assert_eq!(unsafe { sqlite3_open(memory.as_ptr(), &mut a) }, SQLITE_OK);
    assert_eq!(unsafe { sqlite3_open(memory.as_ptr(), &mut b) }, SQLITE_OK);
    assert_eq!(
        exec(a, "CREATE TABLE t(x); INSERT INTO t VALUES (1), (2)"),
        SQLITE_OK
    );
    assert_eq!(count(a, "SELECT count(*) FROM t"), Some(2));
    assert_eq!(
        count(b, "SELECT count(*) FROM t"),
        None,
        "b must not see a's table"
    );
    assert_eq!(
        filename(a),
        "",
        "in-memory databases report an empty filename"
    );
    // Backup into an in-memory destination is refused rather than copied
    // into the working directory through the empty filename.
    let main = c"main".as_ptr();
    assert!(unsafe { sqlite3_backup_init(b, main, a, main) }.is_null());
    assert_eq!(unsafe { sqlite3_close(a) }, SQLITE_OK);
    assert_eq!(unsafe { sqlite3_close(b) }, SQLITE_OK);

    // The native entry point and open_v2 follow the same rule; an in-memory
    // database needs no SQLITE_OPEN_CREATE.
    let mut native = ptr::null_mut();
    assert_eq!(
        unsafe { rldb_open(memory.as_ptr(), &mut native) },
        SQLITE_OK
    );
    assert_eq!(unsafe { rldb_close(native) }, SQLITE_OK);
    let mut v2: *mut sqlite3 = ptr::null_mut();
    let rc =
        unsafe { sqlite3_open_v2(memory.as_ptr(), &mut v2, SQLITE_OPEN_READWRITE, ptr::null()) };
    assert_eq!(rc, SQLITE_OK);
    assert_eq!(unsafe { sqlite3_close(v2) }, SQLITE_OK);

    // SQLITE_OPEN_MEMORY opens in memory whatever the name.
    let named = c"named-memory-db";
    let mut flagged: *mut sqlite3 = ptr::null_mut();
    let flags = SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_MEMORY;
    assert_eq!(
        unsafe { sqlite3_open_v2(named.as_ptr(), &mut flagged, flags, ptr::null()) },
        SQLITE_OK
    );
    assert_eq!(exec(flagged, "CREATE TABLE t(x)"), SQLITE_OK);
    assert_eq!(unsafe { sqlite3_close(flagged) }, SQLITE_OK);

    // URI filenames are not interpreted. With SQLITE_OPEN_URI a "file:" name
    // is refused rather than opened as a literal path.
    let uri: *const c_char = c"file::memory:".as_ptr();
    let mut refused: *mut sqlite3 = ptr::null_mut();
    let flags = SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_URI;
    assert_eq!(
        unsafe { sqlite3_open_v2(uri, &mut refused, flags, ptr::null()) },
        SQLITE_CANTOPEN
    );
    assert!(refused.is_null());

    assert_eq!(
        entries(cwd.path()),
        Vec::<String>::new(),
        "opening in-memory databases must not create files in the working directory"
    );
}
