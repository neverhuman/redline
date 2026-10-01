use std::fs;
use std::process::Command;

use serde_json::Value;

#[test]
fn strict_cli_emits_seven_complete_adjacent_control_pairs() {
    let scratch = tempfile::tempdir().unwrap();
    let binary = env!("CARGO_BIN_EXE_redline-scoreboard");
    let invoke = |reps: &str| {
        Command::new(binary)
            .args([
                "strict-pair",
                "--version",
                &format!("v1={binary}"),
                "--version",
                &format!("v2={binary}"),
                "--run",
                "2",
                "--rows",
                "10",
                "--reps",
                reps,
                "--workloads",
                "update_pk_1txn,delete_pk_1txn",
                "--image-store",
            ])
            .arg(scratch.path().join("images"))
            .arg("--work-root")
            .arg(scratch.path().join("work"))
            .arg("--out")
            .arg(scratch.path().join("out"))
            .output()
            .unwrap()
    };
    let too_short = invoke("3");
    assert!(!too_short.status.success());
    assert!(String::from_utf8_lossy(&too_short.stderr).contains("at least 7"));
    let result = invoke("7");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut records: Vec<Value> = ["v1", "v2"]
        .iter()
        .flat_map(|label| {
            let path = scratch
                .path()
                .join("out")
                .join(label)
                .join("run-2-strict.jsonl");
            fs::read_to_string(path)
                .unwrap()
                .lines()
                .map(|s| serde_json::from_str(s).unwrap())
                .collect::<Vec<Value>>()
        })
        .collect();
    records.sort_by_key(|r| r["seq"].as_u64().unwrap());
    assert_eq!(records.len(), 2 * 7 * 4);
    for (seq, record) in records.iter().enumerate() {
        assert_eq!(record["seq"], seq);
        assert_eq!(record["status"], "ok");
        assert_eq!(record["work_divisor"], 1);
        assert_eq!(record["rows"], 10);
    }
    for group in records.chunks_exact(4) {
        let rep = group[0]["rep"].as_u64().unwrap();
        let first = if rep.is_multiple_of(2) { "v2" } else { "v1" };
        let second = if first == "v1" { "v2" } else { "v1" };
        for (record, (owner, label)) in group.iter().zip([
            (first, first),
            (first, "sqlite"),
            (second, "sqlite"),
            (second, second),
        ]) {
            assert_eq!(record["with"], owner);
            assert_eq!(record["label"], label);
            assert_eq!(record["rep"], rep);
            assert_eq!(record["digest"], group[0]["digest"]);
        }
    }
    // Immutable raw outputs cannot be silently appended or replaced.
    assert!(!invoke("7").status.success());
}
