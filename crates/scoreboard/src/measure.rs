//! Process resource use around a timed region: CPU time, peak memory and
//! I/O, read from the kernel.

use serde::Serialize;

/// CPU time and peak resident memory of this process.
#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    pub user_ns: u64,
    pub sys_ns: u64,
    pub max_rss_kib: u64,
}

pub fn usage() -> Usage {
    // SAFETY: valid initializer for `libc::rusage`, a POSIX Copy POD of
    // integer fields whose all-zero bit pattern is a valid value; `raw` is a
    // fresh stack local. Ledgered in .jankurai/unsafe-ledger.toml
    // (crates/scoreboard/src/measure.rs, rust.unsafe.zeroed).
    let mut raw: libc::rusage = unsafe { std::mem::zeroed::<libc::rusage>() };
    // SAFETY: RUSAGE_SELF cannot fail with EINVAL, and `&mut raw` is a valid,
    // aligned, writable pointer with exclusive access; on rc == 0 the kernel
    // writes every public field (`man 2 getrusage`). Ledgered likewise.
    let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut raw as *mut libc::rusage) };
    if rc != 0 {
        return Usage::default();
    }
    let ns = |tv: libc::timeval| tv.tv_sec as u64 * 1_000_000_000 + tv.tv_usec as u64 * 1_000;
    Usage {
        user_ns: ns(raw.ru_utime),
        sys_ns: ns(raw.ru_stime),
        // Linux reports ru_maxrss in KiB.
        max_rss_kib: raw.ru_maxrss as u64,
    }
}

/// Bytes and calls this process read and wrote (`/proc/self/io`).
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Io {
    pub rchar: u64,
    pub wchar: u64,
    pub syscr: u64,
    pub syscw: u64,
    pub write_bytes: u64,
}

pub fn io() -> Io {
    let Ok(text) = std::fs::read_to_string("/proc/self/io") else {
        return Io::default();
    };
    let mut io = Io::default();
    for line in text.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().parse().unwrap_or(0);
        match name {
            "rchar" => io.rchar = value,
            "wchar" => io.wchar = value,
            "syscr" => io.syscr = value,
            "syscw" => io.syscw = value,
            "write_bytes" => io.write_bytes = value,
            _ => {}
        }
    }
    io
}

impl Io {
    pub fn since(self, earlier: Io) -> Io {
        Io {
            rchar: self.rchar.saturating_sub(earlier.rchar),
            wchar: self.wchar.saturating_sub(earlier.wchar),
            syscr: self.syscr.saturating_sub(earlier.syscr),
            syscw: self.syscw.saturating_sub(earlier.syscw),
            write_bytes: self.write_bytes.saturating_sub(earlier.write_bytes),
        }
    }
}

/// Latency percentiles in nanoseconds.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Latency {
    pub samples: u64,
    pub p50: u64,
    pub p95: u64,
    pub p99: u64,
}

pub fn latency(mut samples: Vec<u64>) -> Option<Latency> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_unstable();
    let at = |q: f64| {
        let index = ((samples.len() as f64 - 1.0) * q).round() as usize;
        samples[index.min(samples.len() - 1)]
    };
    Some(Latency {
        samples: samples.len() as u64,
        p50: at(0.50),
        p95: at(0.95),
        p99: at(0.99),
    })
}

/// Per-key difference of two JSON objects of counters; keys missing from
/// `before` count from zero. Engines of different versions expose
/// different counters, so nothing assumes a fixed set.
pub fn counters_since(after: &serde_json::Value, before: &serde_json::Value) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    if let Some(after) = after.as_object() {
        for (key, value) in after {
            let now = value.as_u64().unwrap_or(0);
            let then = before.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
            out.insert(key.clone(), serde_json::json!(now.saturating_sub(then)));
        }
    }
    serde_json::Value::Object(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn rusage_reports_this_process() {
        let spin: u64 = (0..2_000_000u64).fold(0, |acc, i| acc.wrapping_add(i * i));
        std::hint::black_box(spin);
        let usage = super::usage();
        assert!(usage.max_rss_kib > 0);
        assert!(usage.user_ns + usage.sys_ns > 0);
    }
}
