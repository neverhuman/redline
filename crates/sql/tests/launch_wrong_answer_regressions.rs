//! Launch wrong-answer regressions (Phase 4, SQL correctness L-06).
//!
//! Each module pins one audited wrong answer against bundled SQLite with
//! strict storage-class and error-class comparison (see `lab.rs`). New
//! regressions add a module under `launch_wrong_answer/` so no file grows
//! past the 2,000-line cap.

#[path = "launch_wrong_answer/index_epoch_upgrade.rs"]
mod index_epoch_upgrade;
#[path = "launch_wrong_answer/index_numeric_keys.rs"]
mod index_numeric_keys;
#[path = "launch_wrong_answer/lab.rs"]
mod lab;
#[path = "launch_wrong_answer/numeric.rs"]
mod numeric;
#[path = "launch_wrong_answer/q5_05_text_ops.rs"]
mod q5_05_text_ops;
#[path = "launch_wrong_answer/reindex.rs"]
mod reindex;
#[path = "launch_wrong_answer/sum_overflow.rs"]
mod sum_overflow;
