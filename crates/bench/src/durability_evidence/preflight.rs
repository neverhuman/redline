//! Facts a receipt records before any scenario runs: what the binary is,
//! whether it is a failpoint build, which toolchain and source it came
//! from, and which filesystem the databases live on.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use super::receipt::{
    BinaryFacts, DeviceFacts, FailpointScan, FilesystemFacts, HostFacts, ToolchainFacts,
};

/// Strings only a failpoint build contains: every name the kernel passes to
/// `fail_point!` or `io_error_under` (the macro and helper compile to
/// nothing without the `failpoints` feature), plus panic and parse strings
/// of the `fail` crate itself. `tests/durability_evidence_schema.rs` checks
/// this list against `crates/kernel/src`, so a new failpoint cannot slip
/// past the scan.
pub const FAILPOINT_MARKERS: &[&str] = &[
    "catalog::save::fsync",
    "catalog::save::parent_fsync",
    "catalog::save::rename",
    "catalog::save::temp_write",
    "engine::checkpoint",
    "engine::commit::before_publish",
    "engine::recovery::after_heap_replay",
    "heap::mutation",
    "index::delete",
    "index::insert",
    "index::split",
    "index::split_image",
    "integrity::heap::visible",
    "integrity::index::dump",
    "integrity::page_csum::sweep",
    "storage::control::write",
    "vector::hnsw::insert::after_link",
    "vector::hnsw::page_image",
    "vector::hnsw::search::beam_step",
    "wal::append_commit_error",
    "wal::flush",
    "wal::flush_all",
    "wal::flush_error",
    "wal::flush_until",
    "wal::pipeline::before_fdatasync",
    "wal::pipeline::before_writev",
    "wal::prune",
    "wal::salvage_error",
    "wal::sync_dir",
    "wal::write_encoded",
    "wal::write_error",
    // The `fail` crate (0.5) with its `failpoints` feature.
    "failed to parse frequency",
    "sleep require timeout",
];

pub struct BinaryScan {
    pub facts: BinaryFacts,
    pub failpoints: FailpointScan,
}

pub fn scan_binary(binary: &Path) -> Result<BinaryScan> {
    let bytes = fs::read(binary).with_context(|| format!("read binary {}", binary.display()))?;
    let markers_found: Vec<String> = FAILPOINT_MARKERS
        .iter()
        .filter(|marker| contains(&bytes, marker.as_bytes()))
        .map(|marker| (*marker).to_owned())
        .collect();
    let facts = BinaryFacts {
        path: binary.display().to_string(),
        sha256: sha256_hex(&bytes),
        bytes: bytes.len() as u64,
        version_line: version_line(binary),
        elf_machine: elf_machine(&bytes),
        rustc_commit: embedded_rustc_commit(&bytes),
    };
    Ok(BinaryScan {
        facts,
        failpoints: FailpointScan {
            clean: markers_found.is_empty(),
            markers_checked: FAILPOINT_MARKERS.len(),
            markers_found,
        },
    })
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn version_line(binary: &Path) -> Option<String> {
    let out = Command::new(binary)
        .arg("--version")
        .env_clear()
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
}

fn elf_machine(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 20 || &bytes[..4] != b"\x7fELF" {
        return None;
    }
    let raw = match bytes[5] {
        1 => u16::from_le_bytes([bytes[18], bytes[19]]),
        _ => u16::from_be_bytes([bytes[18], bytes[19]]),
    };
    Some(match raw {
        0x3e => "x86_64".to_owned(),
        0xb7 => "aarch64".to_owned(),
        0xf3 => "riscv".to_owned(),
        other => format!("machine {other:#x}"),
    })
}

/// Rust binaries carry `/rustc/<40 hex>/library/...` paths of the compiler
/// that built their standard library.
fn embedded_rustc_commit(bytes: &[u8]) -> Option<String> {
    let needle = b"/rustc/";
    let mut from = 0;
    while let Some(pos) = bytes[from..]
        .windows(needle.len())
        .position(|window| window == needle)
    {
        let start = from + pos + needle.len();
        let hash = bytes.get(start..start + 40)?;
        if hash.iter().all(u8::is_ascii_hexdigit) && bytes.get(start + 40) == Some(&b'/') {
            return Some(String::from_utf8_lossy(hash).into_owned());
        }
        from = start;
    }
    None
}

/// `rustc -vV` run in `repo`, so its `rust-toolchain.toml` applies.
pub fn toolchain(repo: &Path) -> ToolchainFacts {
    let run = |args: &[&str]| {
        Command::new("rustc")
            .args(args)
            .current_dir(repo)
            .stdin(Stdio::null())
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let verbose = run(&["-vV"]).unwrap_or_default();
    let field = |name: &str| {
        verbose
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .map(|value| value.trim().to_owned())
    };
    ToolchainFacts {
        rustc: run(&["-V"]).map(|line| line.trim().to_owned()),
        rustc_commit: field("commit-hash:"),
        target_triple: field("host:"),
    }
}

pub fn host() -> HostFacts {
    let read = |name: &str| {
        fs::read_to_string(format!("/proc/sys/kernel/{name}"))
            .ok()
            .map(|text| text.trim().to_owned())
    };
    let kernel = match (read("ostype"), read("osrelease")) {
        (Some(os), Some(release)) => Some(match read("version") {
            Some(version) => format!("{os} {release} {version}"),
            None => format!("{os} {release}"),
        }),
        _ => None,
    };
    HostFacts {
        kernel,
        arch: std::env::consts::ARCH.to_owned(),
        logical_cpus: std::thread::available_parallelism().map_or(1, usize::from),
    }
}

/// `HEAD` of `repo` and whether any tracked file is modified or any
/// untracked file exists. An unreadable tree counts as dirty.
pub fn source(repo: &Path) -> (Option<String>, bool) {
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let sha = git(&["rev-parse", "HEAD"]).map(|text| text.trim().to_owned());
    let dirty = match git(&["status", "--porcelain", "--untracked-files=normal"]) {
        Some(status) => !status.trim().is_empty(),
        None => true,
    };
    (sha, dirty)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNTINFO: &str = "\
22 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw,errors=remount-ro
27 22 0:26 / /dev/shm rw,nosuid,nodev shared:4 - tmpfs tmpfs rw,inode64
40 22 259:3 / /data rw,noatime shared:20 - xfs /dev/nvme1n1 rw,attr2
41 40 0:40 / /data/scratch rw,relatime shared:21 - tmpfs tmpfs rw,size=1024k
42 22 0:41 / /mnt/with\\040space rw shared:22 - ext4 /dev/sdb1 rw
43 40 259:3 / /data rw,noatime shared:23 - btrfs /dev/nvme1n1 rw
";

    #[test]
    fn mount_for_picks_the_longest_prefix_and_the_later_of_two_equal_ones() {
        let root = mount_for(MOUNTINFO, Path::new("/home/user/db")).expect("root");
        assert_eq!(root.fs_type, "ext4");
        assert_eq!(root.mount_point, PathBuf::from("/"));
        let shm = mount_for(MOUNTINFO, Path::new("/dev/shm/rt/db")).expect("shm");
        assert_eq!(shm.fs_type, "tmpfs");
        let scratch = mount_for(MOUNTINFO, Path::new("/data/scratch/x")).expect("scratch");
        assert_eq!(scratch.fs_type, "tmpfs");
        // `/data` is mounted twice; the later mount hides the earlier one.
        let data = mount_for(MOUNTINFO, Path::new("/data/db")).expect("data");
        assert_eq!(data.fs_type, "btrfs");
        // A sibling whose name only shares a prefix is not under the mount.
        let sibling = mount_for(MOUNTINFO, Path::new("/data2/db")).expect("sibling");
        assert_eq!(sibling.fs_type, "ext4");
        let spaced = mount_for(MOUNTINFO, Path::new("/mnt/with space/db")).expect("spaced");
        assert_eq!(spaced.source, "/dev/sdb1");
        assert_eq!(spaced.mount_options, "rw");
    }

    #[test]
    fn a_line_without_the_separator_is_skipped() {
        assert_eq!(
            parse_mountinfo_line("22 1 259:2 / / rw ext4 /dev/x rw"),
            None
        );
    }

    #[test]
    fn embedded_rustc_commit_needs_forty_hex_digits_and_a_slash() {
        let hash = "59807616e1fa2540724bfbac14d7976d7e4a3860";
        let bytes = format!("junk /rustc/short/ more /rustc/{hash}/library/core/src/x.rs");
        assert_eq!(
            embedded_rustc_commit(bytes.as_bytes()),
            Some(hash.to_owned())
        );
        assert_eq!(embedded_rustc_commit(b"/rustc/nothing here"), None);
    }
}
