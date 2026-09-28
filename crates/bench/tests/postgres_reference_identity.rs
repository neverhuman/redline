//! The PostgreSQL reference identity the parity scripts hand the runner
//! (PG-05): the image digest is measured from the running container, never
//! filled in because the server's settings look right, and every place that
//! names the reference image names the same pinned digest.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const PIN: &str = "sha256:efdf07c2f9d4df592783dcc8ea5f6db02efbf5f6452b527225ff5e58364570e9";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &str) -> String {
    let path = repository_root().join(path);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// A `docker` stand-in: `ps` prints `$STUB_CID` for `--filter publish=$STUB_PORT`,
/// `inspect` prints `$STUB_IMAGE`, `image inspect` prints `$STUB_REPO_DIGESTS`.
fn stub_docker(dir: &Path) {
    let script = r#"#!/bin/bash
case "$1" in
  ps)
    for arg in "$@"; do
      if [ "$arg" = "publish=${STUB_PORT}" ]; then printf '%s\n' "${STUB_CID}"; fi
    done ;;
  inspect) printf '%s\n' "${STUB_IMAGE}" ;;
  image) printf '%s\n' ${STUB_REPO_DIGESTS} ;;
  *) exit 1 ;;
esac
"#;
    let path = dir.join("docker");
    fs::write(&path, script).expect("write docker stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod stub");
    }
}

/// Sources ops/ci/lib.sh, runs the measurement with `env`, and returns the
/// exported `IMAGE|SOURCE`.
fn measure(env: &[(&str, &str)], with_docker: bool) -> String {
    let stub = tempfile::tempdir().expect("stub dir");
    let bin = stub.path().join("bin");
    fs::create_dir_all(&bin).expect("stub bin");
    // A PATH holding only what the function runs, so a real docker on the
    // host cannot answer for the stub.
    for tool in ["dirname", "sed", "head"] {
        let found = ["/usr/bin", "/bin"]
            .iter()
            .map(|dir| Path::new(dir).join(tool))
            .find(|path| path.is_file())
            .unwrap_or_else(|| panic!("{tool} not found"));
        #[cfg(unix)]
        std::os::unix::fs::symlink(found, bin.join(tool)).expect("link tool");
    }
    if with_docker {
        stub_docker(&bin);
    }
    let path = bin.display().to_string();
    let mut command = Command::new("/bin/bash");
    command
        .arg("-c")
        .arg(
            ". ops/ci/lib.sh && ci_measure_postgres_reference_image >/dev/null 2>&1 \
             && printf '%s|%s' \"${REDLINE_TESTING_POSTGRES_IMAGE:-}\" \"${REDLINE_TESTING_POSTGRES_IMAGE_SOURCE:-}\"",
        )
        .current_dir(repository_root())
        .env_clear()
        .env("PATH", path)
        .env("HOME", stub.path());
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().expect("run bash");
    assert!(
        output.status.success(),
        "measurement failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf8")
}

const URL: &str = "postgres://redlinedb:postgres@127.0.0.1:55432/redlinedb_beyond";

#[test]
fn the_image_digest_is_measured_from_the_container_that_publishes_the_port() {
    let running = |image: &str, digests: &str| {
        measure(
            &[
                ("REDLINE_TESTING_POSTGRES_URL", URL),
                ("STUB_PORT", "55432"),
                ("STUB_CID", "d642c31f0db9"),
                ("STUB_IMAGE", image),
                ("STUB_REPO_DIGESTS", digests),
            ],
            true,
        )
    };
    // Image id equal to the pin (containerd store) or a repo digest that is.
    assert_eq!(running(PIN, ""), format!("{PIN}|measured"));
    assert_eq!(
        running("sha256:config", &format!("postgres@{PIN}")),
        format!("{PIN}|measured")
    );
    // Another image is recorded as what it is, so the gate refuses it.
    assert_eq!(
        running("sha256:other", "postgres@sha256:other"),
        "sha256:other|measured"
    );
}

#[test]
fn a_digest_that_cannot_be_measured_is_marked_asserted() {
    // A preset digest survives, but is labelled asserted, whatever the
    // caller claimed about its source.
    for (env, docker) in [
        // No docker on PATH.
        (vec![("REDLINE_TESTING_POSTGRES_URL", URL)], false),
        // No container publishes the port.
        (
            vec![
                ("REDLINE_TESTING_POSTGRES_URL", URL),
                ("STUB_PORT", "1"),
                ("STUB_CID", "x"),
            ],
            true,
        ),
        // Not a local URL.
        (
            vec![(
                "REDLINE_TESTING_POSTGRES_URL",
                "postgres://u:p@db.example:5432/redlinedb_beyond",
            )],
            true,
        ),
    ] {
        let mut env = env;
        env.push(("REDLINE_TESTING_POSTGRES_IMAGE", PIN));
        env.push(("REDLINE_TESTING_POSTGRES_IMAGE_SOURCE", "measured"));
        assert_eq!(measure(&env, docker), format!("{PIN}|asserted"), "{env:?}");
    }
}

#[test]
fn every_script_names_the_same_pinned_reference_image() {
    let lib = read("ops/ci/lib.sh");
    assert!(lib.contains(&format!("ci_postgres_reference_image_pin=\"{PIN}\"")));
    for workflow in [
        ".github/workflows/ci.yml",
        ".github/workflows/sqlite-parity-report.yml",
    ] {
        assert!(
            read(workflow).contains(&format!("postgres:16.15-bookworm@{PIN}")),
            "{workflow}"
        );
    }
    let reference = read("ops/ci/beyond-postgres-reference.sh");
    assert!(reference.contains(&format!("postgres:16.15-bookworm@{PIN}")));
    assert!(!reference.contains("postgres:16-alpine"));
    // parity.sh no longer fills the digest in because the settings matched.
    let parity = read("ops/ci/parity.sh");
    assert!(!parity.contains(PIN), "parity.sh asserts the image digest");
    assert!(parity.contains("ci_measure_postgres_reference_image"));
    let lane = read("scripts/just/run.sh");
    assert!(lane.contains("ci_measure_postgres_reference_image"));
    assert!(lane.contains("--expected-source-commit"));
    let publish = read("ops/ci/sqlite-parity-report.sh");
    assert!(publish.contains("--require-clean"));
    assert!(publish.contains("--expected-source-commit"));
}
