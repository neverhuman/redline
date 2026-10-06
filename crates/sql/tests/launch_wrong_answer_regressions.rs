//! Launch wrong-answer regressions (Phase 4, SQL correctness L-06).
//!
//! Each module pins one audited wrong answer against bundled SQLite with
//! strict storage-class and error-class comparison (see `lab.rs`). New
//! regressions add a module under `launch_wrong_answer/` so no file grows
//! past the 2,000-line cap.

#[path = "launch_wrong_answer/comparison_affinity.rs"]
mod comparison_affinity;
#[path = "launch_wrong_answer/index_covering_class.rs"]
mod index_covering_class;
#[path = "launch_wrong_answer/index_epoch_upgrade.rs"]
mod index_epoch_upgrade;
#[path = "launch_wrong_answer/index_numeric_keys.rs"]
mod index_numeric_keys;
#[path = "launch_wrong_answer/index_point_order.rs"]
mod index_point_order;
#[path = "launch_wrong_answer/lab.rs"]
mod lab;
#[path = "launch_wrong_answer/new_01_order_by_ordinal.rs"]
mod new_01_order_by_ordinal;
#[path = "launch_wrong_answer/new_02_group_numeric.rs"]
mod new_02_group_numeric;
#[path = "launch_wrong_answer/numeric.rs"]
mod numeric;
#[path = "launch_wrong_answer/q5_01_literals.rs"]
mod q5_01_literals;
#[path = "launch_wrong_answer/q5_01_quoted_forms.rs"]
mod q5_01_quoted_forms;
#[path = "launch_wrong_answer/q5_02_merge_unique.rs"]
mod q5_02_merge_unique;
#[path = "launch_wrong_answer/q5_02_partial_update.rs"]
mod q5_02_partial_update;
#[path = "launch_wrong_answer/q5_02_reentry.rs"]
mod q5_02_reentry;
#[path = "launch_wrong_answer/q5_02_unique_point.rs"]
mod q5_02_unique_point;
#[path = "launch_wrong_answer/q5_03_recursive_limit.rs"]
mod q5_03_recursive_limit;
#[path = "launch_wrong_answer/q5_04_set_keys.rs"]
mod q5_04_set_keys;
#[path = "launch_wrong_answer/q5_05_synthetic_affinity.rs"]
mod q5_05_synthetic_affinity;
#[path = "launch_wrong_answer/q5_05_text_ops.rs"]
mod q5_05_text_ops;
#[path = "launch_wrong_answer/reindex.rs"]
mod reindex;
#[path = "launch_wrong_answer/rowid_range_bounds.rs"]
mod rowid_range_bounds;
#[path = "launch_wrong_answer/sum_overflow.rs"]
mod sum_overflow;
#[path = "launch_wrong_answer/window_frames.rs"]
mod window_frames;
