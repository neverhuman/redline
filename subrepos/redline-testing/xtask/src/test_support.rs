//! Fixtures for xtask tests that write fake executables and run them.
//!
//! Two hazards come with that. First, `std::env::temp_dir()` may be mounted
//! noexec, so the fixtures live beside the test binary under the target
//! directory. Second, another test thread that forks while a fixture is
//! still open for writing hands its child that write descriptor until the
//! child execs; exec'ing the fixture then fails with ETXTBSY ("Text file
//! busy", rust-lang/rust#114554). `write_executable` therefore runs the new
//! file once, retrying while it is busy, before handing it to the test:
//! once one exec has succeeded, no write descriptor is left anywhere.

use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A scratch directory under the target directory, removed on drop.
pub(crate) struct ScratchDir(PathBuf);

impl ScratchDir {
    pub(crate) fn new(label: &str) -> Self {
        let exe = std::env::current_exe().expect("test binary path");
        let base = exe
            .parent()
            .and_then(Path::parent)
            .expect("test binary under target/<profile>/deps")
            .join("xtask-test-tmp");
        let path = base.join(format!(
            "{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("scratch dir");
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Writes `script` to `path` as an executable and waits until it runs.
pub(crate) fn write_executable(path: &Path, script: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("bin dir");
    fs::write(path, script).expect("write executable");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod");
    for _ in 0..500 {
        match Command::new(path)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Err(error) if error.kind() == ErrorKind::ExecutableFileBusy => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("run {}: {error}", path.display()),
            Ok(_) => return,
        }
    }
    panic!("{} stayed busy", path.display());
}
