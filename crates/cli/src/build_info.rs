//! `redlinedb --build-info [--json]`: which build this binary is.
//!
//! `scripts/package-release.sh` sets `REDLINEDB_BUILD_TAG` and
//! `REDLINEDB_BUILD_SHA` while it compiles a release; rustc records both as
//! inputs of this crate, so changing either rebuilds it. A development build
//! reports no tag and the source `unknown`. The release workflow's
//! `verify-published` job installs the published archive and compares this
//! output with the tag and commit it released (`scripts/release/verify-published.sh`).

use serde_json::json;

/// Schema of the JSON form. Bump it when a field changes meaning.
const SCHEMA: &str = "redline.build-info/v1";
/// The one repository allowed to build and publish releases. It must match
/// `ops/release/authority.env`; `crates/cli/tests/build_info.rs` checks.
const REPOSITORY_URL: &str = "https://github.com/neverhuman/redline";
const REPOSITORY_ID: u64 = 1_390_165_945;
/// The SQLite release the parity oracle is built from
/// (`scripts/sqlite/build-reference.sh`); checked by the same test.
const SQLITE_ORACLE: &str = "3.53.1";

fn source_sha() -> &'static str {
    option_env!("REDLINEDB_BUILD_SHA").unwrap_or("unknown")
}

fn tag() -> Option<&'static str> {
    option_env!("REDLINEDB_BUILD_TAG")
}

fn target() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

/// The report, one line of JSON or labelled lines, newline-terminated.
pub(crate) fn render(as_json: bool) -> String {
    if as_json {
        let value = json!({
            "schema": SCHEMA,
            "version": env!("CARGO_PKG_VERSION"),
            "tag": tag(),
            "source_sha": source_sha(),
            "target": target(),
            "repository_url": REPOSITORY_URL,
            "repository_id": REPOSITORY_ID,
            "sqlite_oracle": SQLITE_ORACLE,
        });
        format!("{value}\n")
    } else {
        format!(
            "redlinedb {version}\ntag: {tag}\nsource: {sha}\ntarget: {target}\n\
             repository: {url} (id {id})\nsqlite oracle: {oracle}\n",
            version = env!("CARGO_PKG_VERSION"),
            tag = tag().unwrap_or("none"),
            sha = source_sha(),
            target = target(),
            url = REPOSITORY_URL,
            id = REPOSITORY_ID,
            oracle = SQLITE_ORACLE,
        )
    }
}
