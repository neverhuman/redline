//! `redline-scoreboard`: RedlineDB and SQLite, in-process, on the same fixed
//! work, from the same images, with matched durability and cache.
//!
//! `run` drives a whole run and writes one JSON record per repetition;
//! `image` and `case` are the child processes it starts. `list` prints the
//! workload catalog. A bundle of runs across versions is assembled and
//! summarized by `scripts/perf/scoreboard-bench.sh`.

pub mod case;
pub mod cli;
pub mod driver;
pub mod image;
pub mod measure;
pub mod pair;
pub mod redline;
pub mod render;
pub mod run;
pub mod sqlite;
pub mod summary;
pub mod workloads;

#[cfg(test)]
mod tests;
