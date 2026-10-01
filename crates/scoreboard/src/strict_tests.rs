use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::pair::Pair;

fn bundle() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("scratch");
    fs::write(
        dir.path().join("bundle.json"),
        json!({
            "schema": "redline-scoreboard-bundle-v1", "bundle": "paired-test",
            "labels": ["v1", "v2"], "rows": 100, "runs": 3, "reps": 5,
            "pairs": ["strict"], "strict_reps": 7, "strict_method": "paired-repetition-v1"
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        dir.path().join("host.json"),
        json!({"normal_dir_fs":"tmpfs"}).to_string(),
    )
    .unwrap();
    let mut host = String::new();
    for run in 1..=3 {
        for label in ["v1", "v2"] {
            host.push_str(
                &json!({
                    "label":label, "run":run, "pair":"strict", "waited_out":false,
                    "runner_jobs_before":0, "runner_jobs_after":0, "runner_jobs_max":0,
                    "loadavg_max":4, "max_loadavg":16
                })
                .to_string(),
            );
            host.push('\n');
        }
    }
    fs::write(dir.path().join("runs.jsonl"), host).unwrap();
    for label in ["v1", "v2"] {
        fs::create_dir(dir.path().join(label)).unwrap();
    }
    for run in 1..=3 {
        let mut files = [String::new(), String::new()];
        let mut seq = 0;
        for workload in crate::run::selected(&None, Pair::Strict).unwrap() {
            for rep in 0..7 {
                let first = if (run + rep) % 2 == 1 { "v1" } else { "v2" };
                let second = if first == "v1" { "v2" } else { "v1" };
                for (label, engine) in [
                    (first, first),
                    (first, "sqlite"),
                    (second, "sqlite"),
                    (second, second),
                ] {
                    let phase = if rep % 3 == 0 { 1.4 } else { 1.0 };
                    let seconds = match engine {
                        "sqlite" => phase * if label == "v1" { 1.0 } else { 1.03 },
                        "v1" => 4.0,
                        _ => 2.0,
                    };
                    let record = json!({
                        "status":"ok", "label":engine, "with":label, "run":run,
                        "rep":rep, "seq":seq, "pair":"strict", "workload":workload.id,
                        "rows":100, "ops":100, "work_divisor":1, "digest":42,
                        "elapsed_ns":(seconds * 1e9) as u64,
                        "engine_version":if engine=="sqlite" {"sqlite 3.50.2"} else {"redlinedb test"}
                    });
                    let index = usize::from(label == "v2");
                    files[index].push_str(&record.to_string());
                    files[index].push('\n');
                    seq += 1;
                }
            }
        }
        for (index, label) in ["v1", "v2"].iter().enumerate() {
            fs::write(
                dir.path()
                    .join(label)
                    .join(format!("run-{run}-strict.jsonl")),
                &files[index],
            )
            .unwrap();
        }
    }
    dir
}

fn edit(path: &Path, change: impl FnOnce(&mut Vec<Value>)) {
    let mut rows: Vec<Value> = fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    change(&mut rows);
    let mut text = rows
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    text.push('\n');
    fs::write(path, text).unwrap();
}

#[test]
fn strict_paired_controls_reject_missing_or_separated_repetitions() {
    let valid = bundle();
    let summary = crate::summary::summarize(valid.path()).unwrap();
    assert!(summary.publishable, "{:?}", summary.blockers);
    for defect in ["missing", "duplicate", "separated", "short"] {
        let broken = bundle();
        let file = broken.path().join("v2/run-1-strict.jsonl");
        edit(&file, |rows| {
            let index = rows
                .iter()
                .position(|r| r["label"] == "sqlite" && r["rep"] == 0)
                .unwrap();
            match defect {
                "missing" => {
                    rows.remove(index);
                }
                "duplicate" => {
                    rows[index]["rep"] = json!(1);
                }
                "separated" => {
                    rows[index]["seq"] = json!(9000);
                }
                "short" => {
                    rows.retain(|r| r["rep"] != 6);
                }
                _ => unreachable!(),
            }
        });
        let result = crate::summary::summarize(broken.path()).unwrap();
        assert!(!result.publishable, "{defect} was accepted");
        assert!(
            result
                .blockers
                .iter()
                .any(|b| b.contains("engine/control pair")),
            "{:?}",
            result.blockers
        );
    }
}

#[test]
fn strict_drift_uses_adjacent_pairs_and_rejects_a_whole_drifted_run() {
    let valid = bundle();
    // One phase outlier among seven does not become a different-version
    // host window; the run median still measures the other six pairs.
    edit(&valid.path().join("v2/run-1-strict.jsonl"), |rows| {
        for row in rows
            .iter_mut()
            .filter(|r| r["label"] == "sqlite" && r["rep"] == 0)
        {
            row["elapsed_ns"] = json!(row["elapsed_ns"].as_u64().unwrap() * 2);
        }
    });
    assert!(crate::summary::summarize(valid.path()).unwrap().publishable);
    edit(&valid.path().join("v2/run-2-strict.jsonl"), |rows| {
        for row in rows.iter_mut().filter(|r| r["label"] == "sqlite") {
            row["elapsed_ns"] = json!(row["elapsed_ns"].as_u64().unwrap() * 2);
        }
    });
    let result = crate::summary::summarize(valid.path()).unwrap();
    assert!(!result.publishable);
    assert!(
        result
            .blockers
            .iter()
            .any(|b| b.contains("adjacent SQLite controls"))
    );
}

#[test]
fn strict_requires_the_declared_repetition_count() {
    let dir = bundle();
    let path = dir.path().join("bundle.json");
    let mut manifest: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    manifest["strict_reps"] = json!(9);
    fs::write(&path, manifest.to_string()).unwrap();
    assert!(!crate::summary::summarize(dir.path()).unwrap().publishable);
    manifest["strict_reps"] = json!(3);
    fs::write(path, manifest.to_string()).unwrap();
    let result = crate::summary::summarize(dir.path()).unwrap();
    assert!(result.blockers.iter().any(|b| b.contains("at least 7")));
}

#[test]
fn strict_ratios_are_medians_of_same_repetition_ratios() {
    let dir = bundle();
    for run in 1..=3 {
        for label in ["v1", "v2"] {
            edit(
                &dir.path()
                    .join(label)
                    .join(format!("run-{run}-strict.jsonl")),
                |rows| {
                    for row in rows {
                        let rep = row["rep"].as_u64().unwrap() as usize;
                        let own = [1, 1, 1, 1, 4, 4, 4][rep];
                        let control = [1, 1, 1, 2, 4, 4, 4][rep];
                        row["elapsed_ns"] = json!(if row["label"] == "sqlite" {
                            control * 1_000_000_000u64
                        } else {
                            own * 1_000_000_000u64
                        });
                    }
                },
            );
        }
    }
    let result = crate::summary::summarize(dir.path()).unwrap();
    assert!(result.publishable, "{:?}", result.blockers);
    assert_eq!(result.workloads[0].vs_sqlite["v1"].median, Some(1.0));
    // Dividing separate medians would incorrectly give 2 / 1.
}

#[test]
fn strict_metadata_does_not_relax_normal_control_drift() {
    let dir = bundle();
    let path = dir.path().join("bundle.json");
    let mut manifest: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    manifest["pairs"] = json!(["normal", "strict"]);
    fs::write(path, manifest.to_string()).unwrap();
    let hosts: Vec<Value> = fs::read_to_string(dir.path().join("runs.jsonl"))
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    let mut normal_hosts = fs::read_to_string(dir.path().join("runs.jsonl")).unwrap();
    for mut host in hosts {
        host["pair"] = json!("normal");
        normal_hosts.push_str(&host.to_string());
        normal_hosts.push('\n');
    }
    fs::write(dir.path().join("runs.jsonl"), normal_hosts).unwrap();
    for label in ["v1", "v2"] {
        for run in 1..=3 {
            let mut text = String::new();
            for workload in crate::run::selected(&None, Pair::Normal).unwrap() {
                for rep in 0..5 {
                    for engine in [label, "sqlite"] {
                        text.push_str(&json!({
                            "status":"ok", "label":engine, "with":label, "run":run,
                            "rep":rep, "pair":"normal", "workload":workload.id,
                            "rows":100, "ops":100, "work_divisor":1, "digest":42,
                            "elapsed_ns":if engine=="sqlite" && label=="v2" {2_000_000_000u64} else {1_000_000_000u64},
                            "engine_version":if engine=="sqlite" {"sqlite 3.50.2"} else {"redlinedb test"}
                        }).to_string());
                        text.push('\n');
                    }
                }
            }
            fs::write(
                dir.path()
                    .join(label)
                    .join(format!("run-{run}-normal.jsonl")),
                text,
            )
            .unwrap();
        }
    }
    let result = crate::summary::summarize(dir.path()).unwrap();
    assert!(!result.publishable);
    assert!(
        result
            .blockers
            .iter()
            .any(|b| b.starts_with("normal ") && b.contains("SQLite differed"))
    );
    assert!(
        !result.blockers.iter().any(|b| b.starts_with("strict ")),
        "{:?}",
        result.blockers
    );
}
