#[cfg(feature = "wal_cross_lane_coalescer")]
pub mod coalescer;
pub mod combiner;
pub mod lanes;
pub mod manager;
pub mod payload;
#[cfg(feature = "wal_pipeline")]
pub mod pipeline;
pub(crate) mod policy;
pub mod record;
pub mod segment;

#[cfg(test)]
thread_local! {
    static BEFORE_PAGE_INSTALL: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        std::cell::RefCell::new(None);
}

/// Test hook that runs after an index WAL append and before the leaf install.
#[cfg(test)]
pub(crate) fn set_before_page_install_hook(hook: Option<Box<dyn FnMut()>>) {
    BEFORE_PAGE_INSTALL.with(|slot| *slot.borrow_mut() = hook);
}

#[cfg(test)]
pub(crate) fn run_before_page_install_hook() {
    let hook = BEFORE_PAGE_INSTALL.with(|slot| slot.borrow_mut().take());
    if let Some(mut hook) = hook {
        hook();
    }
}

#[cfg(test)]
thread_local! {
    static BEFORE_COMMIT_PUBLISH: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        std::cell::RefCell::new(None);
}

/// Test hook that runs after the commit record is durable and before publish.
#[cfg(test)]
pub(crate) fn set_before_commit_publish_hook(hook: Option<Box<dyn FnMut()>>) {
    BEFORE_COMMIT_PUBLISH.with(|slot| *slot.borrow_mut() = hook);
}

#[cfg(test)]
pub(crate) fn run_before_commit_publish_hook() {
    let hook = BEFORE_COMMIT_PUBLISH.with(|slot| slot.borrow_mut().take());
    if let Some(mut hook) = hook {
        hook();
    }
}

pub use lanes::{LaneRoundRobin, WalLaneCoordinator, WalLaneRecoveryReport};
pub use manager::*;
pub use payload::*;
pub use record::*;
pub use segment::*;
