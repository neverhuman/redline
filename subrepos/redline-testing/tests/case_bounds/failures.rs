//! A target killed by a signal, the processes a case leaves behind, and the
//! records a run wrote before it failed.

use super::*;

#[test]
fn signalled_target_fails_its_case() {
    let fixture = Fixture::new();
    let target = fixture.target("cat >/dev/null\nkill -SEGV $$");
    let run = fixture.run(&target, &["00001"], &[], &[]);
    let record = run.only_record("00001");
    assert_eq!(record["status"], "failed", "{record}");
    assert_eq!(record["execution_outcome"], "signal", "{record}");
    assert_eq!(record["target_execution_outcome"], "signal", "{record}");
    assert_eq!(record["target_exit_code"], Value::Null, "{record}");
}

#[test]
fn no_process_of_a_case_survives_it() {
    let fixture = Fixture::new();
    let mark = marker();
    // A child that holds the output pipes and a detached grandchild that
    // does not, left behind by a target that exits normally.
    let target = fixture.target(
        "sleep \"997.$FAKE_MARK\" &\n( sleep \"998.$FAKE_MARK\" </dev/null >/dev/null 2>&1 & )\ncat >/dev/null\nexit 0",
    );
    let run = fixture.run(&target, &["00001"], &[], &[("FAKE_MARK", &mark)]);
    assert!(
        run.elapsed < Duration::from_secs(30),
        "the runner waited {:?} on the target's children",
        run.elapsed
    );
    run.only_record("00001");
    assert_no_survivors(&format!("99[78][.]{mark}"));

    // And a background child of a target killed at the deadline.
    let target = fixture.target("sleep \"996.$FAKE_MARK\" &\nexec sleep 30");
    let run = fixture.run(
        &target,
        &["00001"],
        &["--case-timeout-ms", "500"],
        &[("FAKE_MARK", &mark)],
    );
    assert_eq!(run.only_record("00001")["execution_outcome"], "timeout");
    assert_no_survivors(&format!("996[.]{mark}"));
}

#[test]
fn earlier_records_stay_readable_after_a_mid_run_failure() {
    let fixture = Fixture::new();
    let counter = fixture.path().join("invocations");
    let raw = fixture.path().join("raw.jsonl");
    // The first case runs the real shell; the second kills the runner once
    // the first case's record is on disk (or after 5 s without it).
    let target = fixture.target(
        r#"n=$(( $(cat "$FAKE_COUNTER" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$FAKE_COUNTER"
if [ "$n" -ge 2 ]; then
  for _ in $(seq 1 500); do [ -s "$FAKE_RAW" ] && break; sleep 0.01; done
  kill -KILL "$PPID"
  exit 0
fi
exec "$FAKE_SQLITE" -batch -bail "$3""#,
    );
    let sqlite = sqlite3();
    let run = fixture.run(
        &target,
        &["00001", "00002", "00003"],
        &[],
        &[
            ("FAKE_COUNTER", counter.to_str().expect("utf8 path")),
            ("FAKE_RAW", raw.to_str().expect("utf8 path")),
            ("FAKE_SQLITE", sqlite.to_str().expect("utf8 path")),
        ],
    );
    assert_eq!(
        run.status.signal(),
        Some(9),
        "the fake target kills the runner mid-run; stderr:\n{}",
        run.stderr
    );
    // The completed case's record was streamed before the failure and
    // reads back as a whole record.
    let record = run.only_record("00001");
    assert_eq!(record["sample_role"], "measured:1", "{record}");
    assert!(
        run.records
            .iter()
            .all(|record| record["case_id"] == "00001"),
        "only the completed case has records: {:?}",
        run.records
    );
    // An interrupted run is never marked complete.
    assert!(run.completion_marker().is_none());
}
