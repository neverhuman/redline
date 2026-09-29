//! The raw JSONL of one suite, written as its cases complete (SQ-09).
//!
//! Workers hand each finished case's records to one writer thread, which
//! appends and flushes them at once: a run that dies part way (a harness
//! error, a panic, a killed runner) leaves every completed case's records
//! readable, and never a half-written case. Only a run that finished writes
//! the completion marker `<raw>.complete.json`, which names the raw file's
//! SHA-256 and its record and case counts; the evidence processor and the
//! report require it, so an interrupted run's records are never published.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub const COMPLETION_SCHEMA: &str = "redline-testing-raw-complete-v1";

/// `<raw>.complete.json`, beside the raw file it certifies.
pub fn completion_marker_path(raw: &Path) -> PathBuf {
    let name = raw
        .file_name()
        .map_or_else(|| "raw".into(), |name| name.to_string_lossy().into_owned());
    raw.with_file_name(format!("{name}.complete.json"))
}

/// What a finished run certifies about its raw file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompletionMarker {
    pub schema_version: &'static str,
    pub suite: String,
    pub raw_file: String,
    pub records: usize,
    pub cases: usize,
    pub raw_sha256: String,
}

#[derive(Debug, Default)]
struct Totals {
    records: usize,
    cases: usize,
}

pub struct RecordSink {
    path: PathBuf,
    suite: String,
    sender: Option<mpsc::Sender<Vec<u8>>>,
    writer: Option<JoinHandle<Result<Totals>>>,
}

impl RecordSink {
    /// Appends to `path` (the caller truncates it) and removes any marker a
    /// previous run left beside it.
    pub fn open(path: &Path, suite: &str) -> Result<Self> {
        let marker = completion_marker_path(path);
        if marker.exists() {
            fs::remove_file(&marker).with_context(|| {
                format!("remove the leftover completion marker {}", marker.display())
            })?;
        }
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("create parent for {}", path.display()))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("open raw records {}", path.display()))?;
        let (sender, receiver) = mpsc::channel::<Vec<u8>>();
        let display = path.display().to_string();
        let writer = thread::spawn(move || {
            let mut totals = Totals::default();
            for chunk in receiver {
                file.write_all(&chunk)
                    .and_then(|()| file.flush())
                    .with_context(|| format!("append raw records to {display}"))?;
                totals.records += chunk.iter().filter(|byte| **byte == b'\n').count();
                totals.cases += 1;
            }
            file.sync_all()
                .with_context(|| format!("sync raw records {display}"))?;
            Ok(totals)
        });
        Ok(Self {
            path: path.to_path_buf(),
            suite: suite.to_owned(),
            sender: Some(sender),
            writer: Some(writer),
        })
    }

    /// Queues one case's records, all of them or none, for the writer.
    pub fn write_case<T: Serialize>(&self, records: &[T]) -> Result<()> {
        let mut chunk = Vec::new();
        for record in records {
            serde_json::to_writer(&mut chunk, record)?;
            chunk.push(b'\n');
        }
        self.sender
            .as_ref()
            .ok_or_else(|| anyhow!("raw record writer already finished"))?
            .send(chunk)
            .map_err(|_| anyhow!("the raw record writer for {} stopped", self.path.display()))
    }

    /// Waits for every queued record, then writes the completion marker.
    pub fn finish(mut self) -> Result<CompletionMarker> {
        drop(self.sender.take());
        let totals = self
            .writer
            .take()
            .ok_or_else(|| anyhow!("raw record writer already finished"))?
            .join()
            .map_err(|_| anyhow!("raw record writer panicked"))??;
        let bytes =
            fs::read(&self.path).with_context(|| format!("read {}", self.path.display()))?;
        let marker = CompletionMarker {
            schema_version: COMPLETION_SCHEMA,
            suite: self.suite.clone(),
            raw_file: self
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            records: totals.records,
            cases: totals.cases,
            raw_sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        let marker_path = completion_marker_path(&self.path);
        let staged = marker_path.with_extension("json.partial");
        let mut file =
            File::create(&staged).with_context(|| format!("create {}", staged.display()))?;
        file.write_all(format!("{}\n", serde_json::to_string_pretty(&marker)?).as_bytes())
            .and_then(|()| file.sync_all())
            .with_context(|| format!("write {}", staged.display()))?;
        fs::rename(&staged, &marker_path)
            .with_context(|| format!("publish {}", marker_path.display()))?;
        Ok(marker)
    }
}

impl Drop for RecordSink {
    /// A sink dropped without `finish` (the run failed) still lets its
    /// writer append everything already queued, and writes no marker.
    fn drop(&mut self) {
        drop(self.sender.take());
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_sink_certifies_exactly_its_records() {
        let dir = tempfile::tempdir().expect("tempdir");
        let raw = dir.path().join("suite.raw.jsonl");
        fs::write(completion_marker_path(&raw), "stale").expect("stale marker");
        let sink = RecordSink::open(&raw, "sqlite_parity").expect("open");
        assert!(
            !completion_marker_path(&raw).exists(),
            "a stale marker must not survive a new run"
        );
        sink.write_case(&[
            serde_json::json!({"case_id": "00001"}),
            serde_json::json!({"case_id": "00001"}),
        ])
        .expect("case 1");
        sink.write_case(&[serde_json::json!({"case_id": "00002"})])
            .expect("case 2");
        let marker = sink.finish().expect("finish");
        assert_eq!((marker.records, marker.cases), (3, 2));
        let bytes = fs::read(&raw).expect("raw");
        assert_eq!(marker.raw_sha256, format!("{:x}", Sha256::digest(&bytes)));
        assert_eq!(marker.raw_file, "suite.raw.jsonl");
        let written: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(completion_marker_path(&raw)).expect("marker"),
        )
        .expect("marker json");
        assert_eq!(written["schema_version"], COMPLETION_SCHEMA);
        assert_eq!(written["records"], 3);
    }

    #[test]
    fn unfinished_sink_keeps_its_records_and_writes_no_marker() {
        let dir = tempfile::tempdir().expect("tempdir");
        let raw = dir.path().join("suite.raw.jsonl");
        let sink = RecordSink::open(&raw, "memory").expect("open");
        sink.write_case(&[serde_json::json!({"case_id": "00001"})])
            .expect("case");
        drop(sink);
        assert_eq!(
            fs::read_to_string(&raw).expect("raw"),
            "{\"case_id\":\"00001\"}\n"
        );
        assert!(!completion_marker_path(&raw).exists());
    }
}
