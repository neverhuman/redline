//! Give the C shared library its ABI-major identity.
//!
//! The major comes from `RLDB_ABI_MAJOR` in `contracts/c-abi/redlinedb.h`,
//! so the header, the soname and the packaged file names cannot disagree.
//! Linux-style targets get the soname `libredlinedb.so.<major>`; macOS gets
//! the install name `@rpath/libredlinedb.<major>.dylib`. Only the cdylib is
//! affected; the staticlib and rlib have no load identity.

use std::path::Path;

fn main() {
    let header = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/c-abi/redlinedb.h");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", header.display());
    let text = std::fs::read_to_string(&header)
        .unwrap_or_else(|error| panic!("read {}: {error}", header.display()));
    let major: u32 = text
        .lines()
        .find_map(|line| line.strip_prefix("#define RLDB_ABI_MAJOR "))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or_else(|| panic!("{} must define RLDB_ABI_MAJOR", header.display()));
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target_os.as_str() {
        "linux" | "android" | "freebsd" | "netbsd" | "openbsd" | "dragonfly" => {
            println!("cargo:rustc-cdylib-link-arg=-Wl,-soname,libredlinedb.so.{major}");
        }
        "macos" => {
            println!(
                "cargo:rustc-cdylib-link-arg=-Wl,-install_name,@rpath/libredlinedb.{major}.dylib"
            );
        }
        _ => {}
    }
}
