//! `sqlite-qualification.json` must come from the verified runner and this
//! run's evidence and sample plan, and match the raw records case for case.

use super::*;

const RUNNER: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

struct Run {
    dir: tempfile::TempDir,
    official: Value,
    official_bytes: Vec<u8>,
    qualification: Value,
    validated: Map<String, Value>,
}

impl Run {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let raw = concat!(
            "{\"case_id\":\"00001\",\"status\":\"passed\"}\n",
            "{\"case_id\":\"00002\",\"status\":\"passed\"}\n",
            "{\"case_id\":\"00002\",\"status\":\"failed\"}\n",
            "{\"case_id\":\"00003\",\"status\":\"skipped\"}\n",
        );
        fs::write(dir.path().join("raw.jsonl"), raw).expect("raw");
        let official = json!({"warmup": 1, "repetitions": 3});
        let official_bytes = official.to_string().into_bytes();
        let qualification = json!({
            "schema_version": QUALIFICATION_SCHEMA,
            "runner": {"binary_sha256": RUNNER},
            "official_evidence_sha256": format!("{:x}", Sha256::digest(&official_bytes)),
            "warmup": 1, "repetitions": 3,
            "suites": {"memory": {
                "raw_sha256": format!("{:x}", Sha256::digest(raw.as_bytes())),
                "total": 3, "passed": 1, "failed": 1, "skipped": 1,
                "passed_case_ids": ["00001"], "failed_case_ids": ["00002"],
                "skipped_case_ids": ["00003"],
            }},
        });
        let validated = json!({"total": 3, "passed": 1, "failed": 1, "skipped": 1})
            .as_object()
            .expect("object")
            .clone();
        Self {
            dir,
            official,
            official_bytes,
            qualification,
            validated,
        }
    }

    fn check(&self) -> Result<()> {
        fs::write(
            self.dir.path().join(QUALIFICATION_FILE),
            self.qualification.to_string(),
        )
        .expect("qualification");
        let (qualification, _) = load(
            self.dir.path(),
            &self.official,
            &self.official_bytes,
            RUNNER,
        )?;
        check_suite(
            &qualification,
            "memory",
            &self.validated,
            &self.dir.path().join("raw.jsonl"),
            3,
        )
    }
}

#[test]
fn qualification_must_match_the_raw_records() {
    Run::new().check().expect("a consistent reduction");
    type Edit = fn(&mut Value);
    let broken: [(&str, Edit); 9] = [
        ("overlapping verdicts", |q| {
            q["suites"]["memory"]["passed_case_ids"] = json!(["00002"]);
        }),
        ("a replaced case", |q| {
            q["suites"]["memory"]["passed_case_ids"] = json!(["00009"]);
        }),
        ("a failed case called passed", |q| {
            q["suites"]["memory"]["failed_case_ids"] = json!(["00001"]);
            q["suites"]["memory"]["passed_case_ids"] = json!(["00002"]);
        }),
        ("ids that disagree with a count", |q| {
            q["suites"]["memory"]["passed_case_ids"] = json!(["00001", "00001"]);
        }),
        ("other raw bytes", |q| {
            q["suites"]["memory"]["raw_sha256"] = json!("0".repeat(64));
        }),
        ("another runner", |q| {
            q["runner"]["binary_sha256"] = json!("f".repeat(64));
        }),
        ("another run's evidence", |q| {
            q["official_evidence_sha256"] = json!("0".repeat(64));
        }),
        ("another sample plan", |q| {
            q["repetitions"] = json!(1);
        }),
        ("a missing suite", |q| {
            q["suites"] = json!({});
        }),
    ];
    for (what, edit) in broken {
        let mut run = Run::new();
        edit(&mut run.qualification);
        assert!(run.check().is_err(), "{what} was accepted");
    }
    // The published counts must be the reduction's, and the corpus size.
    let mut run = Run::new();
    run.validated.insert("passed".to_owned(), json!(2));
    assert!(run.check().is_err());
    let run = Run::new();
    fs::write(
        run.dir.path().join(QUALIFICATION_FILE),
        run.qualification.to_string(),
    )
    .expect("qualification");
    let (qualification, summary) =
        load(run.dir.path(), &run.official, &run.official_bytes, RUNNER).expect("load");
    assert_eq!(summary["schema_version"], QUALIFICATION_SCHEMA);
    assert!(
        check_suite(
            &qualification,
            "memory",
            &run.validated,
            &run.dir.path().join("raw.jsonl"),
            2_445
        )
        .is_err()
    );
}
