//! The build contract records the measured binaries as they are (BM3-05).

use std::fs;
use std::os::unix::fs::PermissionsExt;

use super::*;

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, format!("#!/usr/bin/env bash\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    wait_until_executable(&path);
    path
}

/// A child another test thread forked while `path` was open for writing
/// holds a write descriptor until it execs, and exec'ing `path` then fails
/// with ETXTBSY. Once one exec succeeds, none is left.
fn wait_until_executable(path: &Path) {
    for _ in 0..500 {
        match Command::new(path)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Err(error) if error.raw_os_error() == Some(26) => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            _ => return,
        }
    }
    panic!("{} stayed busy", path.display());
}

fn input(dir: &Path, build: DeclaredBuild) -> BuildContractInput {
    BuildContractInput {
        output: dir.join("out").join("build-contract.json"),
        target_bin: script(dir, "redlinedb", "echo 'redlinedb 9.9.9-fixture'"),
        reference_bin: script(
            dir,
            "sqlite3",
            r#"if [ "$1" = --version ]; then echo '3.53.1 fixture'; else printf 'COMPILER=gcc\nENABLE_FTS5\n\nTHREADSAFE=1\n'; fi"#,
        ),
        runner_bin: script(dir, "redline-testing", "echo 'redline-testing 1.0.1'"),
        build,
        pgo_training_corpus: None,
    }
}

#[test]
fn contract_records_the_measured_binaries() {
    let dir = tempfile::tempdir().unwrap();
    let input = input(dir.path(), DeclaredBuild::default());
    let contract = write_build_contract(&input).expect("contract");
    assert_eq!(contract.schema_version, BUILD_CONTRACT_SCHEMA);
    assert_eq!(contract.target.identity.version, "redlinedb 9.9.9-fixture");
    assert_eq!(
        contract.target.identity.sha256,
        sha256_file(&input.target_bin).unwrap()
    );
    assert_eq!(contract.reference.identity.version, "3.53.1 fixture");
    assert_eq!(
        contract.reference.compile_options,
        ["COMPILER=gcc", "ENABLE_FTS5", "THREADSAFE=1"]
    );
    assert_eq!(contract.runner.version, "redline-testing 1.0.1");
    assert!(
        contract
            .toolchain
            .rustc_verbose_version
            .iter()
            .any(|line| line.starts_with("rustc ")),
        "{:?}",
        contract.toolchain.rustc_verbose_version
    );
    // Nothing declared: nothing guessed, and no training corpus.
    let written: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&input.output).unwrap()).unwrap();
    assert_eq!(
        written["target"]["build"],
        serde_json::json!({"declared": false, "profile": null, "features": null, "rustflags": null})
    );
    assert_eq!(
        written["optimization"]["pgo_training_corpus"],
        serde_json::Value::Null
    );
    assert_eq!(
        written["target"]["path"],
        fs::canonicalize(&input.target_bin)
            .unwrap()
            .display()
            .to_string()
    );
}

#[test]
fn declared_empty_rustflags_differ_from_undeclared() {
    let dir = tempfile::tempdir().unwrap();
    let declared = DeclaredBuild {
        declared: true,
        profile: Some("release".to_owned()),
        features: Some("--no-default-features --features alloc-mimalloc".to_owned()),
        rustflags: Some(String::new()),
    };
    let contract = capture_build_contract(&input(dir.path(), declared.clone())).unwrap();
    assert_eq!(contract.target.build, declared);
    let json = serde_json::to_value(&contract).unwrap();
    assert_eq!(json["target"]["build"]["rustflags"], "");

    // Build fields without the declaration are refused.
    let inconsistent = DeclaredBuild {
        declared: false,
        profile: Some("release".to_owned()),
        ..DeclaredBuild::default()
    };
    assert!(capture_build_contract(&input(dir.path(), inconsistent)).is_err());
}

#[test]
fn a_reference_that_cannot_list_its_options_fails() {
    let dir = tempfile::tempdir().unwrap();
    let mut input = input(dir.path(), DeclaredBuild::default());
    input.reference_bin = script(
        dir.path(),
        "broken-sqlite3",
        r#"if [ "$1" = --version ]; then echo '3.53.1 fixture'; else echo 'no pragma' >&2; exit 1; fi"#,
    );
    let error = capture_build_contract(&input).unwrap_err();
    assert!(
        format!("{error:#}").contains("PRAGMA compile_options"),
        "{error:#}"
    );
}
