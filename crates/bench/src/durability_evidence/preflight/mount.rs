//! The filesystem the databases live on, read from `/proc/self/mountinfo`,
//! with best-effort facts about the block device under it.

use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};

use crate::durability_evidence::receipt::{DeviceFacts, FilesystemFacts};

/// One line of `/proc/self/mountinfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    pub mount_point: PathBuf,
    pub mount_options: String,
    pub fs_type: String,
    pub source: String,
    pub super_options: String,
}

pub fn parse_mountinfo_line(line: &str) -> Option<MountEntry> {
    let fields: Vec<&str> = line.split(' ').collect();
    let dash = fields.iter().position(|field| *field == "-")?;
    if dash < 6 || fields.len() < dash + 4 {
        return None;
    }
    Some(MountEntry {
        mount_point: PathBuf::from(unescape(fields[4])),
        mount_options: fields[5].to_owned(),
        fs_type: fields[dash + 1].to_owned(),
        source: unescape(fields[dash + 2]),
        super_options: fields[dash + 3].to_owned(),
    })
}

/// The mount `path` (canonical, absolute) lives on: the longest mount
/// point that is a prefix of it, the later line winning a tie, as a later
/// mount hides an earlier one at the same point.
pub fn mount_for(mountinfo: &str, path: &Path) -> Option<MountEntry> {
    let mut best: Option<(usize, MountEntry)> = None;
    for entry in mountinfo.lines().filter_map(parse_mountinfo_line) {
        if !path.starts_with(&entry.mount_point) {
            continue;
        }
        let depth = entry
            .mount_point
            .components()
            .filter(|c| matches!(c, Component::Normal(_)))
            .count();
        if best
            .as_ref()
            .is_none_or(|(best_depth, _)| depth >= *best_depth)
        {
            best = Some((depth, entry));
        }
    }
    best.map(|(_, entry)| entry)
}

fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1..i + 4]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b))
        {
            let value =
                (bytes[i + 1] - b'0') * 64 + (bytes[i + 2] - b'0') * 8 + (bytes[i + 3] - b'0');
            out.push(value);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The filesystem holding `work_dir` (which must exist), from
/// `/proc/self/mountinfo`.
pub fn filesystem(work_dir: &Path) -> Result<FilesystemFacts> {
    let work_dir = work_dir
        .canonicalize()
        .with_context(|| format!("canonicalize {}", work_dir.display()))?;
    let mountinfo = fs::read_to_string("/proc/self/mountinfo")
        .context("read /proc/self/mountinfo; the receipt must name the filesystem")?;
    let entry = mount_for(&mountinfo, &work_dir).with_context(|| {
        format!(
            "no mount in /proc/self/mountinfo holds {}",
            work_dir.display()
        )
    })?;
    Ok(FilesystemFacts {
        work_dir: work_dir.display().to_string(),
        mount_point: entry.mount_point.display().to_string(),
        device: device(&entry.source),
        fs_type: entry.fs_type,
        source: entry.source,
        mount_options: entry.mount_options,
        super_options: entry.super_options,
    })
}

/// Best-effort block-device facts for a mount source such as
/// `/dev/nvme0n1p2`.
fn device(source: &str) -> Option<DeviceFacts> {
    if !source.starts_with("/dev/") {
        return None;
    }
    let resolved = fs::canonicalize(source).unwrap_or_else(|_| PathBuf::from(source));
    let name = resolved.file_name()?.to_string_lossy().into_owned();
    let class = PathBuf::from("/sys/class/block").join(&name);
    let sys = fs::canonicalize(&class).ok()?;
    let disk = if sys.join("partition").exists() {
        sys.parent()?.to_path_buf()
    } else {
        sys
    };
    let read = |rel: &str| {
        fs::read_to_string(disk.join(rel))
            .ok()
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
    };
    Some(DeviceFacts {
        name,
        model: read("device/model"),
        rotational: read("queue/rotational"),
        write_cache: read("queue/write_cache"),
    })
}
