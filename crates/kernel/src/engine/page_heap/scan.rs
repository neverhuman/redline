//! WS-C3: heap scans for `PageBackedHeap` split across worker threads.
//!
//! An UPDATE or DELETE appends a new tuple and leaves the older one on its
//! page as it was. The row directory names each row's newest tuple, and
//! the older versions are reached through the undo chain, so most tuples on
//! a page can be superseded versions that still look live on their own.
//! These scans therefore start from a snapshot of the row directory, not
//! from the page contents. They group the rows' newest tuples by page; each
//! worker pins its share of those pages through the shared buffer pool
//! (shard-safe per `crates/kernel/src/storage/buffer.rs`), decodes the
//! newest tuples there, and resolves each row as `get_for_relation` does:
//! the tuple itself when the snapshot sees it, nothing for a delete it sees,
//! otherwise the first visible version down the undo chain. A row comes
//! back at most once, from the page its newest tuple is on, and pages only
//! in the buffer pool are read like pages in the file.
//!
//! The `rayon` dep is intentionally NOT pulled into kernel; the SQL-side
//! gate is expected to wrap the call with `pool.install(|| ...)` so the
//! `std::thread::scope` workers run inside the rayon pool's context for
//! NUMA / affinity purposes. Callers that want strict pool ownership can
//! pass `worker_count = pool.current_num_threads()`.
//!
//! Result ordering across pages is **not** stable across runs; rows from
//! the same page are emitted in slot order. Callers that need a
//! deterministic order must sort the returned vector.

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::thread;

use crate::engine::page_heap::PageBackedHeap;
use crate::engine::tx::ConcurrentTxStatus;
use crate::format::{PageId, RelId, RowId, TuplePtr, TupleVersion, TxId};
use crate::txn::Snapshot;
use crate::{Error, Result};

/// One row materialised by [`PageBackedHeap::parallel_scan_page_range`].
/// Mirrors `ConcurrentHeap::parallel_scan`'s shape but carries the
/// `RelId` so callers can dispatch by relation when the heap holds
/// multiple relations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeapScanRow {
    pub rel_id: RelId,
    pub row_id: RowId,
    pub payload: Vec<u8>,
}

/// Diagnostic counters used by the WS-C3 R2 SQL-side gate to surface
/// "how many pages did this scan visit" + "how many fell through visibility"
/// telemetry without snapshotting the buffer-pool stats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ParallelScanDiagnostics {
    /// Pages pinned: those holding the newest tuple of at least one row.
    pub pages_visited: usize,
    /// Pinned pages reused since the directory snapshot. Their rows were
    /// read again through the live row directory.
    pub heap_pages_skipped: usize,
    /// Rows resolved, one per directory entry in the scanned pages.
    pub tuples_seen: usize,
    /// Rows returned.
    pub tuples_visible: usize,
    pub worker_count: usize,
}

/// Build a fresh diagnostics struct. Exposed as a free fn so callers can
/// pre-allocate a single mutable counter and pass it through the scan
/// without rebuilding the struct on every iteration.
pub fn parallel_scan_diagnostics() -> ParallelScanDiagnostics {
    ParallelScanDiagnostics::default()
}

/// A row named by the directory snapshot, located by its newest tuple.
#[derive(Clone, Copy, Debug)]
struct HeadRef {
    rel_id: RelId,
    row_id: RowId,
    ptr: TuplePtr,
}

/// The rows whose newest tuple is on `page_id`, in slot order.
#[derive(Debug)]
struct PageHeads {
    page_id: PageId,
    heads: Vec<HeadRef>,
}

impl PageBackedHeap {
    /// WS-C3 R2: every row visible to `snapshot` whose newest tuple is on a
    /// page in `page_range`, restricted to `rel_filter` (every relation
    /// when `None`), read by `workers` threads.
    ///
    /// `page_range` is a half-open `[start, end)` interval of `PageId`s.
    /// Rows are placed by the page their newest tuple is on, so disjoint
    /// ranges return disjoint rows, and ranges covering
    /// `1..=allocated_page_count()` return each visible row once. A
    /// concurrent UPDATE can move a row's newest tuple to a page allocated
    /// after the caller chose its range; [`Self::parallel_scan_relation`]
    /// takes no range and cannot miss such a row. `workers` is clamped to
    /// the number of pages to read; with one worker the scan runs on the
    /// calling thread.
    ///
    /// Each worker pins one page at a time, drops the guard before
    /// resolving the page's rows, and forwards them on a bounded
    /// `mpsc::sync_channel` so memory stays bounded even when the
    /// consumer is slower than the producers. The drain happens on the
    /// dispatcher thread (the still-serial SQL executor mainline).
    #[allow(clippy::too_many_arguments)]
    pub fn parallel_scan_page_range(
        &self,
        tx_status: &ConcurrentTxStatus,
        snapshot: &Snapshot,
        owner: Option<TxId>,
        page_range: std::ops::Range<PageId>,
        rel_filter: Option<RelId>,
        workers: usize,
        diagnostics: Option<&mut ParallelScanDiagnostics>,
    ) -> Result<Vec<HeapScanRow>> {
        let pages = self.head_pages(rel_filter, Some(page_range))?;
        self.scan_head_pages(tx_status, snapshot, owner, &pages, workers, diagnostics)
    }

    /// Every row of `rel_filter` (every relation when `None`) visible to
    /// `snapshot`, read by `workers` threads that split the pages holding
    /// the rows' newest tuples between them. Each visible row comes back
    /// once, with the payload `get_for_relation` returns for it.
    pub fn parallel_scan_relation(
        &self,
        tx_status: &ConcurrentTxStatus,
        snapshot: &Snapshot,
        owner: Option<TxId>,
        rel_filter: Option<RelId>,
        workers: usize,
        diagnostics: Option<&mut ParallelScanDiagnostics>,
    ) -> Result<Vec<HeapScanRow>> {
        let pages = self.head_pages(rel_filter, None)?;
        self.scan_head_pages(tx_status, snapshot, owner, &pages, workers, diagnostics)
    }

    /// Serial reference scan over a page range: the rows
    /// [`Self::parallel_scan_page_range`] returns, read on the calling
    /// thread in page and slot order.
    pub fn serial_scan_page_range(
        &self,
        tx_status: &ConcurrentTxStatus,
        snapshot: &Snapshot,
        owner: Option<TxId>,
        page_range: std::ops::Range<PageId>,
        rel_filter: Option<RelId>,
        diagnostics: Option<&mut ParallelScanDiagnostics>,
    ) -> Result<Vec<HeapScanRow>> {
        let pages = self.head_pages(rel_filter, Some(page_range))?;
        self.scan_head_pages(tx_status, snapshot, owner, &pages, 1, diagnostics)
    }

    /// Snapshot the row directory for `rel_filter` and group the rows by the
    /// page their newest tuple is on, keeping pages inside `page_range`.
    fn head_pages(
        &self,
        rel_filter: Option<RelId>,
        page_range: Option<std::ops::Range<PageId>>,
    ) -> Result<Vec<PageHeads>> {
        let heads: Vec<HeadRef> = match rel_filter {
            Some(rel_id) => self
                .relation_entries(rel_id)?
                .into_iter()
                .map(|(row_id, ptr)| HeadRef {
                    rel_id,
                    row_id,
                    ptr,
                })
                .collect(),
            None => self
                .all_relation_entries()?
                .into_iter()
                .map(|(rel_id, row_id, ptr)| HeadRef {
                    rel_id,
                    row_id,
                    ptr,
                })
                .collect(),
        };
        let mut by_page: BTreeMap<PageId, Vec<HeadRef>> = BTreeMap::new();
        for head in heads {
            if head.ptr.is_null() {
                return Err(Error::CorruptPage("null tuple pointer"));
            }
            let page = head.ptr.page_id.0;
            if let Some(range) = &page_range
                && (page < range.start.0 || page >= range.end.0)
            {
                continue;
            }
            by_page.entry(head.ptr.page_id).or_default().push(head);
        }
        Ok(by_page
            .into_iter()
            .map(|(page_id, mut heads)| {
                heads.sort_unstable_by_key(|head| head.ptr.slot);
                PageHeads { page_id, heads }
            })
            .collect())
    }

    /// Resolve the rows of `pages`, split across `workers` threads that take
    /// every `workers`-th page each.
    fn scan_head_pages(
        &self,
        tx_status: &ConcurrentTxStatus,
        snapshot: &Snapshot,
        owner: Option<TxId>,
        pages: &[PageHeads],
        workers: usize,
        diagnostics: Option<&mut ParallelScanDiagnostics>,
    ) -> Result<Vec<HeapScanRow>> {
        if pages.is_empty() {
            if let Some(d) = diagnostics {
                *d = ParallelScanDiagnostics::default();
            }
            return Ok(Vec::new());
        }
        let workers = workers.max(1).min(pages.len());
        let (rows, totals) = if workers == 1 {
            let mut rows = Vec::new();
            let mut totals = WorkerDiag::default();
            for page in pages {
                self.collect_page_heads(tx_status, snapshot, owner, page, &mut totals, |row| {
                    rows.push(row)
                })?;
            }
            (rows, totals)
        } else {
            self.scan_head_pages_threaded(tx_status, snapshot, owner, pages, workers)?
        };
        if let Some(d) = diagnostics {
            d.worker_count = workers;
            d.pages_visited = totals.pages_visited;
            d.heap_pages_skipped = totals.heap_pages_skipped;
            d.tuples_seen = totals.tuples_seen;
            d.tuples_visible = rows.len();
        }
        Ok(rows)
    }

    fn scan_head_pages_threaded(
        &self,
        tx_status: &ConcurrentTxStatus,
        snapshot: &Snapshot,
        owner: Option<TxId>,
        pages: &[PageHeads],
        workers: usize,
    ) -> Result<(Vec<HeapScanRow>, WorkerDiag)> {
        // Bounded sync channel so producers throttle when the consumer
        // is slower. Capacity tuned to keep ~16 rows of head-room per
        // worker in flight; the upper bound is intentionally modest so
        // RSS does not balloon on the 1M-row smoke.
        let channel_cap = (workers * 16).max(64);
        let (tx, rx) = mpsc::sync_channel::<HeapScanRow>(channel_cap);
        let (diag_tx, diag_rx) = mpsc::channel::<WorkerDiag>();
        let expected_rows: usize = pages.iter().map(|page| page.heads.len()).sum();

        thread::scope(|scope| -> Result<(Vec<HeapScanRow>, WorkerDiag)> {
            let mut handles = Vec::with_capacity(workers);
            for worker_idx in 0..workers {
                let heap = &*self;
                let tx = tx.clone();
                let diag_tx = diag_tx.clone();
                let handle = scope.spawn(move || -> Result<()> {
                    let mut diag = WorkerDiag::default();
                    for page in pages.iter().skip(worker_idx).step_by(workers) {
                        let collected = heap.collect_page_heads(
                            tx_status,
                            snapshot,
                            owner,
                            page,
                            &mut diag,
                            |row| {
                                // Best-effort send: a closed channel
                                // means the consumer dropped (drained
                                // and bailed early); treat as a clean
                                // stop signal rather than an error.
                                tx.send(row).ok();
                            },
                        );
                        if let Err(err) = collected {
                            let _ = diag_tx.send(diag);
                            return Err(err);
                        }
                    }
                    let _ = diag_tx.send(diag);
                    Ok(())
                });
                handles.push(handle);
            }
            // Drop the dispatcher's clone so `rx.recv` returns `Err`
            // once every worker has finished.
            drop(tx);
            drop(diag_tx);

            // Consumer mainline: drain the channel while workers are
            // still feeding it. This is the "still-serial executor
            // mainline" referenced in the WS-C3 R2 brief.
            let mut rows = Vec::with_capacity(expected_rows);
            while let Ok(row) = rx.recv() {
                rows.push(row);
            }
            let mut totals = WorkerDiag::default();
            while let Ok(d) = diag_rx.recv() {
                totals.add(&d);
            }

            for handle in handles {
                match handle.join() {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => return Err(err),
                    Err(_) => {
                        return Err(Error::CorruptPage(
                            "parallel_scan_page_range worker panicked",
                        ));
                    }
                }
            }

            Ok((rows, totals))
        })
    }

    /// Pin one page, decode the newest tuple of each of its rows, release
    /// the page, then forward the version `snapshot` sees of each row to
    /// `sink`. Resolving happens after the release because reaching an
    /// older version pins undo pages. Errors short-circuit.
    fn collect_page_heads<F>(
        &self,
        tx_status: &ConcurrentTxStatus,
        snapshot: &Snapshot,
        owner: Option<TxId>,
        page: &PageHeads,
        diag: &mut WorkerDiag,
        mut sink: F,
    ) -> Result<()>
    where
        F: FnMut(HeapScanRow),
    {
        let guard = self.buffer_ref().pin(page.page_id)?;
        diag.pages_visited += 1;
        let mut newest: Vec<(HeadRef, TupleVersion)> = Vec::with_capacity(page.heads.len());
        let mut moved: Vec<HeadRef> = Vec::new();
        guard.with_page(|contents| {
            let generation = contents.header()?.generation;
            for head in &page.heads {
                if head.ptr.generation == generation {
                    newest.push((*head, TupleVersion::decode(contents.cell(head.ptr.slot)?)?));
                } else {
                    moved.push(*head);
                }
            }
            Ok(())
        })?;
        drop(guard);

        for (head, tuple) in newest {
            diag.tuples_seen += 1;
            if let Some(payload) =
                self.visible_payload_for_relation(tx_status, snapshot, owner, head.rel_id, tuple)?
            {
                sink(HeapScanRow {
                    rel_id: head.rel_id,
                    row_id: head.row_id,
                    payload,
                });
            }
        }
        if !moved.is_empty() {
            diag.heap_pages_skipped += 1;
        }
        // The page was reused after the directory snapshot, so these rows
        // were vacuumed or have moved. Read each through the live directory,
        // as the serial read does.
        for head in moved {
            diag.tuples_seen += 1;
            if let Some(payload) =
                self.get_for_relation(tx_status, snapshot, owner, head.rel_id, head.row_id)?
            {
                sink(HeapScanRow {
                    rel_id: head.rel_id,
                    row_id: head.row_id,
                    payload,
                });
            }
        }
        Ok(())
    }
}

/// Per-worker counters merged by the dispatcher.
#[derive(Clone, Copy, Debug, Default)]
struct WorkerDiag {
    pages_visited: usize,
    heap_pages_skipped: usize,
    tuples_seen: usize,
}

impl WorkerDiag {
    fn add(&mut self, other: &Self) {
        self.pages_visited += other.pages_visited;
        self.heap_pages_skipped += other.heap_pages_skipped;
        self.tuples_seen += other.tuples_seen;
    }
}
