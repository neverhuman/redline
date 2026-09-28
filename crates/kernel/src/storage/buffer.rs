use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_queue::ArrayQueue;
use crossbeam_utils::CachePadded;

use crate::format::{Lsn, Page, PageId, PageKind, RelId};
use crate::storage::numa;
use crate::storage::policy::{ActiveBufferPolicy, BufferPolicy};
use crate::storage::{PageFile, PagePressureRelief};
use crate::telemetry::Phase11Counters;
use crate::{Error, Result};

#[path = "buffer_checkpoint.rs"]
mod checkpoint;

pub const DEFAULT_CHECKPOINT_BATCH_PAGES: usize = 64;

/// Minimum capacity of the prefetch worker queue. The queue size
/// otherwise scales with the pool (`capacity / 4`), but a tiny pool
/// would otherwise yield a single-slot queue that drops nearly every
/// hint.
const PREFETCH_QUEUE_MIN: usize = 32;
/// How long the worker parks when the queue drains. Park-timeout is
/// woken eagerly on every `try_prefetch` push, so the timeout only
/// matters for shutdown latency and for the (rare) case where an
/// unpark notification is lost.
const PREFETCH_PARK: Duration = Duration::from_micros(200);

/// Buffer pool with a background prefetch worker.
///
/// The previous design ran the prefetch cold load synchronously on the
/// caller's thread, which blocked SQL workers on disk I/O. WS-C4 moves
/// the cold load to a dedicated worker thread fed by a bounded
/// `ArrayQueue`. `try_prefetch` (and `prefetch`) push onto the queue
/// and return immediately; if the queue is full the hint is dropped
/// and `Phase11Counters::prefetch_dropped` is bumped.
///
/// # Drop ordering invariant
///
/// The worker thread MUST NOT hold a strong `Arc<BufferPool>` (or any
/// strong `Arc` into the same cycle), otherwise `Drop for BufferPool`
/// would never run. All pool state lives behind `Arc<Inner>` and the
/// worker upgrades a `Weak<Inner>` per pop. When the last external
/// strong ref drops, `Drop` flips the shutdown flag, unparks the
/// worker, and joins — the worker's next `weak.upgrade()` returns
/// `None` and the loop exits.
#[derive(Debug)]
pub struct BufferPool {
    inner: Arc<Inner>,
    prefetch_queue: Arc<ArrayQueue<PageId>>,
    shutdown: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Debug)]
struct Inner {
    page_file: Arc<PageFile>,
    capacity: usize,
    shards: Vec<Mutex<HashMap<PageId, Arc<FrameEntry>>>>,
    next_page_id: AtomicU64,
    // Phase 5 WS-B5: avoid false-sharing with adjacent counters.
    resident: CachePadded<AtomicUsize>,
    clock_hand: AtomicUsize,
    /// Eviction writes a page without syncing the page file. Set before
    /// such a write, under the eviction mutex, and cleared by the next
    /// checkpoint flush that syncs. A direct write (page-image redo) sets it
    /// too, after the write, so a checkpoint that flushes no frame still
    /// syncs that write before it records a redo LSN past its record.
    evicted_unsynced: AtomicBool,
    /// Asked for a checkpoint when a clock pass finds no page it may evict.
    /// Weak, so the pool never keeps its engine alive.
    relief: OnceLock<Weak<dyn PagePressureRelief>>,
    /// The page file is scratch space no recovery reads, as for a volatile
    /// engine, so eviction may write any unpinned dirty page.
    scratch: AtomicBool,
    eviction: Mutex<()>,
    stats: BufferPoolStatsInner,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BufferPoolStats {
    pub resident_pages: usize,
    pub reads: u64,
    pub writes: u64,
    pub evictions: u64,
    pub checkpoint_flushes: u64,
    /// Checkpoints eviction asked for because no resident page could leave.
    pub pressure_checkpoints: u64,
}

// Phase 5 WS-B5: avoid false-sharing with adjacent counters.
#[derive(Debug, Default)]
struct BufferPoolStatsInner {
    reads: CachePadded<AtomicU64>,
    writes: CachePadded<AtomicU64>,
    evictions: CachePadded<AtomicU64>,
    checkpoint_flushes: CachePadded<AtomicU64>,
    pressure_checkpoints: CachePadded<AtomicU64>,
}

thread_local! {
    /// Set while this thread runs a pressure checkpoint, so that checkpoint
    /// never asks for another one.
    static RELIEVING_PRESSURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[derive(Debug)]
struct FrameEntry {
    state: Mutex<FrameState>,
    ready: Condvar,
}

/// Checkpoints one allocation may ask for before it fails.
const PRESSURE_RELIEF_ROUNDS: usize = 8;

/// Clock hand ceiling. `pin` saturates here; `evict_one` must be allowed
/// this many decays plus one zero-usage visit.
const CLOCK_MAX_USAGE: u8 = 5;

#[derive(Debug)]
pub(crate) struct FrameState {
    pub(crate) page: Option<Page>,
    pin_count: usize,
    pub(crate) dirty: bool,
    usage_count: u8,
    write_in_progress: bool,
    load_failed: bool,
}

#[derive(Debug)]
pub struct PageGuard {
    page_id: PageId,
    frame: Arc<FrameEntry>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlushStats {
    pub flushed_pages: usize,
    pub batches: usize,
}

impl BufferPool {
    pub fn new(page_file: Arc<PageFile>, capacity: usize) -> Result<Self> {
        // A26: hit the process-wide cached available_parallelism() so the
        // BufferPool constructor doesn't re-walk the cgroup hierarchy.
        let parallelism = crate::cached_available_parallelism();
        Self::new_with_parallelism(page_file, capacity, parallelism)
    }

    /// Like `new` but uses a caller-supplied parallelism hint instead of
    /// querying `cached_available_parallelism()`.  Use this for volatile
    /// (in-memory) databases to avoid the cgroup walk on every fresh process.
    pub(crate) fn new_with_parallelism(
        page_file: Arc<PageFile>,
        capacity: usize,
        parallelism: usize,
    ) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::CorruptPage("buffer pool capacity must be nonzero"));
        }
        let base_shard_count = capacity.min((parallelism * 4).max(16)).max(1);
        // Phase 5 WS-B6: with `--features numa` round the shard count up
        // to a multiple of the host's NUMA node count so each node owns
        // a disjoint slab of shards (`shard_idx % nodes == node_id`).
        // Without the feature `numa_node_count()` returns 1 and the
        // round-up is a no-op, so the off-feature build keeps the
        // pre-B6 shard layout byte-identical.
        let nodes = numa::numa_node_count().max(1);
        let shard_count = base_shard_count
            .div_ceil(nodes)
            .saturating_mul(nodes)
            .max(1);
        let mut shards = Vec::with_capacity(shard_count);
        for _ in 0..shard_count {
            shards.push(Mutex::new(HashMap::new()));
        }
        let next_page_id = page_file.page_count()?.saturating_add(1);
        let inner = Arc::new(Inner {
            page_file,
            capacity,
            shards,
            next_page_id: AtomicU64::new(next_page_id),
            resident: CachePadded::new(AtomicUsize::new(0)),
            clock_hand: AtomicUsize::new(0),
            evicted_unsynced: AtomicBool::new(false),
            relief: OnceLock::new(),
            scratch: AtomicBool::new(false),
            eviction: Mutex::new(()),
            stats: BufferPoolStatsInner::default(),
        });

        let queue_capacity = (capacity / 4).max(PREFETCH_QUEUE_MIN);
        let prefetch_queue = Arc::new(ArrayQueue::<PageId>::new(queue_capacity));
        let shutdown = Arc::new(AtomicBool::new(false));

        // Phase 5 hot-fix: prefetch worker is LAZY — spawned on first
        // try_prefetch call instead of at construction. Workloads that
        // never prefetch (in-memory DBs, short-lived CLI scripts) skip
        // the thread-spawn cost entirely (was ~0.2 ms per Database::new
        // × 1127 parity processes = ~225 ms aggregate noise).
        Ok(Self {
            inner,
            prefetch_queue,
            shutdown,
            worker: Mutex::new(None),
        })
    }

    /// Ensure the prefetch worker is spawned. No-op if already spawned.
    fn ensure_prefetch_worker(&self) {
        let mut guard = match self.worker.lock() {
            Ok(g) => g,
            Err(_) => return, // poisoned — skip; worker is best-effort
        };
        if guard.is_some() {
            return;
        }
        let weak_inner: Weak<Inner> = Arc::downgrade(&self.inner);
        let worker_queue = Arc::clone(&self.prefetch_queue);
        let worker_shutdown = Arc::clone(&self.shutdown);
        if let Ok(handle) = thread::Builder::new()
            .name("redlinedb-prefetch".to_string())
            .spawn(move || prefetch_worker(weak_inner, worker_queue, worker_shutdown))
        {
            *guard = Some(handle);
        }
        // On spawn failure we silently leave the worker unset; future
        // try_prefetch calls will simply enqueue with no consumer — the
        // queue overflows and prefetch_dropped fires. Acceptable since
        // prefetch is advisory.
    }

    pub fn allocate(&self, kind: PageKind, rel_id: RelId) -> Result<PageGuard> {
        self.inner.allocate(kind, rel_id)
    }

    /// Attach what eviction asks for a checkpoint when every unpinned frame
    /// holds a dirty page it may not write alone. A pool takes one, once.
    /// Attach it only once the engine is fully open: a checkpoint during
    /// recovery would record an LSN that replay has not reached. See
    /// `Engine::enable_pool_pressure_checkpoints` for when it is safe.
    pub fn attach_pressure_relief(&self, relief: Weak<dyn PagePressureRelief>) -> Result<()> {
        self.inner
            .relief
            .set(relief)
            .map_err(|_| Error::CorruptPage("buffer pool already has pressure relief"))
    }

    /// Treat the page file as scratch space no recovery ever reads, as a
    /// volatile engine's is: eviction may then write any unpinned dirty page
    /// and read it back later, whatever its LSN. Without this a pool whose
    /// pages all carry logged changes can only be relieved by a checkpoint,
    /// which a volatile engine does not take.
    pub(crate) fn use_as_scratch(&self) {
        self.inner.scratch.store(true, Ordering::Release);
    }

    pub(crate) fn page_size(&self) -> usize {
        self.inner.page_file.page_size()
    }

    pub fn pin(&self, page_id: PageId) -> Result<PageGuard> {
        self.inner.pin(page_id)
    }

    /// Push `page_id` onto the prefetch worker queue. Returns
    /// immediately. If the queue is full the hint is dropped and
    /// `Phase11Counters::prefetch_dropped` is bumped. The worker is
    /// unparked on a successful push so latency is bounded by one
    /// thread wake-up, not by the park timeout.
    pub fn try_prefetch(&self, page_id: PageId, counters: &Phase11Counters) {
        // Lazy worker: spawn on first prefetch hint. Subsequent calls
        // hit the fast path (already-Some lock check + unpark).
        self.ensure_prefetch_worker();
        match self.prefetch_queue.push(page_id) {
            Ok(()) => {
                if let Ok(guard) = self.worker.lock()
                    && let Some(handle) = guard.as_ref()
                {
                    handle.thread().unpark();
                }
            }
            Err(_) => {
                counters.prefetch_dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Phase 11 W1-B advisory prefetch hint.
    ///
    /// Resident-hit fast path stays synchronous (a try_lock probe).
    /// Cold cases enqueue onto the worker queue via [`Self::try_prefetch`]
    /// rather than blocking the caller on disk I/O. Counter semantics
    /// for `prefetch_hits`/`prefetch_misses` are unchanged; the new
    /// `prefetch_dropped` counter fires only on queue overflow.
    pub fn prefetch(&self, page_id: PageId, counters: &Phase11Counters) {
        let shard_idx = self.inner.shard_idx(page_id);
        let resident = match self.inner.shards[shard_idx].try_lock() {
            Ok(shard) => shard.contains_key(&page_id),
            Err(_) => {
                // Contended shard: drop the hint as a miss without
                // attempting a cold load.
                counters.prefetch_misses.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        if resident {
            counters.prefetch_hits.fetch_add(1, Ordering::Relaxed);
            return;
        }
        counters.prefetch_misses.fetch_add(1, Ordering::Relaxed);
        if !ActiveBufferPolicy::prefetch_cold_load(self.inner.resident_pages(), self.inner.capacity)
        {
            return;
        }
        // Hand the cold load off to the worker; never block the
        // caller on disk I/O here.
        self.try_prefetch(page_id, counters);
    }

    pub fn flush_page(&self, page_id: PageId, durable_lsn: Lsn) -> Result<()> {
        self.inner.flush_page(page_id, durable_lsn)
    }

    /// Write `page_id` only if eviction would: a dirty page that eviction
    /// must keep resident is left alone. Lets a test choose which pages an
    /// eviction pass reaches.
    #[cfg(test)]
    pub(crate) fn flush_page_if_evictable(&self, page_id: PageId, durable_lsn: Lsn) -> Result<()> {
        let Some(frame) = self.inner.lookup_frame(page_id)? else {
            return Ok(());
        };
        let may_write = {
            let state = frame
                .state
                .lock()
                .map_err(|_| Error::CorruptPage("buffer frame poisoned"))?;
            match state.page.as_ref() {
                Some(page) => !state.dirty || eviction_may_write(page)?,
                None => false,
            }
        };
        if may_write {
            self.inner.flush_frame_if_durable(&frame, durable_lsn)?;
        }
        Ok(())
    }

    pub fn flush_all(&self, durable_lsn: Lsn) -> Result<()> {
        self.inner.flush_all(durable_lsn)
    }

    pub fn write_page_direct(&self, page: &Page) -> Result<()> {
        self.inner.write_page_direct(page)
    }

    pub fn flush_dirty_batch(&self, durable_lsn: Lsn, max_pages: usize) -> Result<FlushStats> {
        self.inner.flush_dirty_batch(durable_lsn, max_pages)
    }

    pub fn flush_dirty_batches(&self, durable_lsn: Lsn, batch_pages: usize) -> Result<FlushStats> {
        self.inner.flush_dirty_batches(durable_lsn, batch_pages)
    }

    pub fn resident_pages(&self) -> usize {
        self.inner.resident_pages()
    }

    pub fn stats(&self) -> BufferPoolStats {
        self.inner.stats()
    }

    pub fn page_count(&self) -> Result<u64> {
        self.inner.page_count()
    }

    /// Lane INT: raw-bytes read used by the integrity checker to recompute
    /// CRC32 numbers when [`pin`] returns [`Error::InvalidChecksum`]. The
    /// regular `pin` path runs `Page::from_bytes`, which validates the
    /// checksum and refuses to surface bytes for a corrupt page.
    pub fn read_page_bytes_unchecked(&self, page_id: PageId) -> Result<Vec<u8>> {
        self.inner.read_page_bytes_unchecked(page_id)
    }
}

impl Drop for BufferPool {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let handle = self.worker.lock().ok().and_then(|mut guard| guard.take());
        if let Some(handle) = handle {
            handle.thread().unpark();
            let _ = handle.join();
        }
    }
}

fn prefetch_worker(
    weak_inner: Weak<Inner>,
    queue: Arc<ArrayQueue<PageId>>,
    shutdown: Arc<AtomicBool>,
) {
    loop {
        if shutdown.load(Ordering::Acquire) {
            return;
        }
        // Drain whatever is queued in a tight loop; only park when
        // the queue is empty. Re-check shutdown each iteration so a
        // shutdown raised while we were draining is honoured before
        // the next park.
        let mut drained_any = false;
        while let Some(page_id) = queue.pop() {
            drained_any = true;
            let Some(inner) = weak_inner.upgrade() else {
                return;
            };
            // pin() errors are swallowed by design — prefetch is
            // advisory and a failed cold load just leaves the page
            // not-warmed.
            let _ = inner.pin(page_id);
            drop(inner);
            if shutdown.load(Ordering::Acquire) {
                return;
            }
        }
        if shutdown.load(Ordering::Acquire) {
            return;
        }
        if !drained_any {
            // Park with a small timeout so a missed unpark eventually
            // wakes us up (defensive — the producer always unparks).
            thread::park_timeout(PREFETCH_PARK);
        }
    }
}

impl Inner {
    fn allocate(&self, kind: PageKind, rel_id: RelId) -> Result<PageGuard> {
        #[cfg(test)]
        super::buffer_test_hooks::take_allocation_failure()?;
        self.ensure_capacity()?;
        let page_id = PageId(self.next_page_id.fetch_add(1, Ordering::Relaxed));
        let page = Page::new(self.page_file.page_size(), kind, page_id, rel_id)?;
        let frame = Arc::new(FrameEntry {
            state: Mutex::new(FrameState {
                page: Some(page),
                pin_count: 1,
                dirty: true,
                usage_count: 1,
                write_in_progress: false,
                load_failed: false,
            }),
            ready: Condvar::new(),
        });
        self.insert_new_frame(page_id, Arc::clone(&frame))?;
        Ok(PageGuard { page_id, frame })
    }

    fn pin(&self, page_id: PageId) -> Result<PageGuard> {
        loop {
            if let Some(frame) = self.lookup_frame(page_id)? {
                #[cfg(test)]
                super::buffer_test_hooks::run_before_pin_lock_hook(page_id);
                let mut state = frame
                    .state
                    .lock()
                    .map_err(|_| Error::CorruptPage("buffer frame poisoned"))?;
                while state.page.is_none() && !state.load_failed {
                    state = frame
                        .ready
                        .wait(state)
                        .map_err(|_| Error::CorruptPage("buffer frame wait poisoned"))?;
                }
                // A failed load, or an eviction since the lookup above, took
                // this frame out of the pool. Look the page up again.
                if state.load_failed {
                    drop(state);
                    continue;
                }
                state.pin_count += 1;
                state.usage_count = state.usage_count.saturating_add(1).min(CLOCK_MAX_USAGE);
                drop(state);
                return Ok(PageGuard { page_id, frame });
            }

            self.ensure_capacity()?;
            let frame = Arc::new(FrameEntry {
                state: Mutex::new(FrameState {
                    page: None,
                    pin_count: 1,
                    dirty: false,
                    usage_count: 1,
                    write_in_progress: false,
                    load_failed: false,
                }),
                ready: Condvar::new(),
            });

            if self.try_insert_loading_frame(page_id, Arc::clone(&frame))? {
                let page = match self.page_file.read_page(page_id) {
                    Ok(page) => page,
                    Err(err) => {
                        self.remove_loading_frame(page_id, &frame)?;
                        return Err(err);
                    }
                };
                self.stats.reads.fetch_add(1, Ordering::Relaxed);
                let mut state = frame
                    .state
                    .lock()
                    .map_err(|_| Error::CorruptPage("buffer frame poisoned"))?;
                state.page = Some(page);
                frame.ready.notify_all();
                drop(state);
                return Ok(PageGuard { page_id, frame });
            }
        }
    }

    fn flush_page(&self, page_id: PageId, durable_lsn: Lsn) -> Result<()> {
        let Some(frame) = self.lookup_frame(page_id)? else {
            return Ok(());
        };
        self.flush_frame(&frame, durable_lsn).map(|flushed| {
            if flushed {
                self.stats
                    .checkpoint_flushes
                    .fetch_add(1, Ordering::Relaxed);
            }
        })
    }

    fn flush_all(&self, durable_lsn: Lsn) -> Result<()> {
        for (_, frame) in self.all_frames()? {
            self.flush_frame(&frame, durable_lsn)?;
        }
        self.sync_flushed_pages(true)
    }

    fn write_page_direct(&self, page: &Page) -> Result<()> {
        self.page_file.write_page(page)?;
        self.evicted_unsynced.store(true, Ordering::Release);
        let page_id = page.header()?.page_id;
        let next = page_id.0.saturating_add(1);
        let mut current = self.next_page_id.load(Ordering::Relaxed);
        while current < next {
            match self.next_page_id.compare_exchange(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
        Ok(())
    }

    fn flush_dirty_batch(&self, durable_lsn: Lsn, max_pages: usize) -> Result<FlushStats> {
        let stats = self.flush_dirty_batch_inner(durable_lsn, max_pages)?;
        self.sync_flushed_pages(stats.flushed_pages > 0)?;
        Ok(stats)
    }

    fn flush_dirty_batches(&self, durable_lsn: Lsn, batch_pages: usize) -> Result<FlushStats> {
        let batch_pages = batch_pages.max(1);
        let mut flushed_pages = 0_usize;
        let mut batches = 0_usize;

        loop {
            let batch = self.flush_dirty_batch_inner(durable_lsn, batch_pages)?;
            if batch.flushed_pages == 0 {
                break;
            }
            flushed_pages += batch.flushed_pages;
            batches += 1;
            if batch.flushed_pages < batch_pages {
                break;
            }
            thread::yield_now();
        }

        self.sync_flushed_pages(flushed_pages > 0)?;

        Ok(FlushStats {
            flushed_pages,
            batches,
        })
    }

    fn flush_dirty_batch_inner(&self, durable_lsn: Lsn, max_pages: usize) -> Result<FlushStats> {
        let max_pages = max_pages.max(1);
        let frames = self.dirty_frames(durable_lsn)?;
        let policy_pages =
            ActiveBufferPolicy::dirty_batch_pages(self.resident_pages(), frames.len()).max(1);
        let page_limit = max_pages.min(policy_pages);
        let mut flushed_pages = 0_usize;
        for frame in frames.into_iter().take(page_limit) {
            if self.flush_frame_if_durable(&frame, durable_lsn)? {
                flushed_pages += 1;
                self.stats
                    .checkpoint_flushes
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        Ok(FlushStats {
            flushed_pages,
            batches: usize::from(flushed_pages > 0),
        })
    }

    /// Sync the page file after a checkpoint flush when this flush wrote a
    /// page or an eviction wrote one since the last sync. The checkpoint that
    /// follows records an LSN past those pages' WAL and prunes it, so an
    /// evicted write has to be durable too. Taking the eviction mutex first
    /// waits out an eviction write still in flight.
    fn sync_flushed_pages(&self, flushed: bool) -> Result<()> {
        drop(
            self.eviction
                .lock()
                .map_err(|_| Error::CorruptPage("buffer eviction mutex poisoned"))?,
        );
        let evicted = self.evicted_unsynced.swap(false, Ordering::AcqRel);
        if !flushed && !evicted {
            return Ok(());
        }
        let synced = self.page_file.sync_data();
        if synced.is_err() && evicted {
            self.evicted_unsynced.store(true, Ordering::Release);
        }
        synced
    }

    fn resident_pages(&self) -> usize {
        self.resident.load(Ordering::Relaxed)
    }

    fn stats(&self) -> BufferPoolStats {
        BufferPoolStats {
            resident_pages: self.resident_pages(),
            reads: self.stats.reads.load(Ordering::Relaxed),
            writes: self.stats.writes.load(Ordering::Relaxed),
            evictions: self.stats.evictions.load(Ordering::Relaxed),
            checkpoint_flushes: self.stats.checkpoint_flushes.load(Ordering::Relaxed),
            pressure_checkpoints: self.stats.pressure_checkpoints.load(Ordering::Relaxed),
        }
    }

    fn page_count(&self) -> Result<u64> {
        self.page_file.page_count()
    }

    fn read_page_bytes_unchecked(&self, page_id: PageId) -> Result<Vec<u8>> {
        self.page_file.read_page_bytes_unchecked(page_id)
    }

    fn insert_new_frame(&self, page_id: PageId, frame: Arc<FrameEntry>) -> Result<()> {
        let shard_idx = self.shard_idx(page_id);
        let mut shard = self.shards[shard_idx]
            .lock()
            .map_err(|_| Error::CorruptPage("buffer shard poisoned"))?;
        if shard.insert(page_id, frame).is_some() {
            return Err(Error::CorruptPage("allocated page already resident"));
        }
        self.resident.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn try_insert_loading_frame(&self, page_id: PageId, frame: Arc<FrameEntry>) -> Result<bool> {
        let shard_idx = self.shard_idx(page_id);
        let mut shard = self.shards[shard_idx]
            .lock()
            .map_err(|_| Error::CorruptPage("buffer shard poisoned"))?;
        if shard.contains_key(&page_id) {
            return Ok(false);
        }
        shard.insert(page_id, frame);
        self.resident.fetch_add(1, Ordering::Relaxed);
        Ok(true)
    }

    fn lookup_frame(&self, page_id: PageId) -> Result<Option<Arc<FrameEntry>>> {
        let shard = self.shards[self.shard_idx(page_id)]
            .lock()
            .map_err(|_| Error::CorruptPage("buffer shard poisoned"))?;
        Ok(shard.get(&page_id).cloned())
    }

    fn ensure_capacity(&self) -> Result<()> {
        if self.resident.load(Ordering::Relaxed) < self.capacity {
            return Ok(());
        }
        let mut reliefs = 0_usize;
        loop {
            {
                let _eviction = self
                    .eviction
                    .lock()
                    .map_err(|_| Error::CorruptPage("buffer eviction mutex poisoned"))?;
                while self.resident.load(Ordering::Relaxed) >= self.capacity {
                    if !self.evict_one()? {
                        break;
                    }
                }
                if self.resident.load(Ordering::Relaxed) < self.capacity {
                    return Ok(());
                }
            }
            // A full clock pass locked every resident frame, so this thread
            // holds none of them, and the eviction mutex is released: the
            // checkpoint below flushes frames and waits on that mutex. Other
            // writers can dirty the frames a checkpoint cleaned before this
            // thread's next pass reaches them, so ask again, a few times.
            if reliefs == PRESSURE_RELIEF_ROUNDS || !self.relieve_pressure()? {
                return Err(Error::CorruptPage(
                    "no unpinned frame available for eviction",
                ));
            }
            reliefs += 1;
        }
    }

    /// Ask the attached engine for a checkpoint. `false` when none is
    /// attached, it is gone, or this thread is already running one.
    fn relieve_pressure(&self) -> Result<bool> {
        let Some(relief) = self.relief.get().and_then(Weak::upgrade) else {
            return Ok(false);
        };
        if RELIEVING_PRESSURE.with(std::cell::Cell::get) {
            return Ok(false);
        }
        struct Relieving;
        impl Drop for Relieving {
            fn drop(&mut self) {
                RELIEVING_PRESSURE.with(|flag| flag.set(false));
            }
        }
        RELIEVING_PRESSURE.with(|flag| flag.set(true));
        let relieving = Relieving;
        let relieved = relief.relieve_page_pressure();
        drop(relieving);
        if matches!(relieved, Ok(true)) {
            self.stats
                .pressure_checkpoints
                .fetch_add(1, Ordering::Relaxed);
        }
        relieved
    }

    fn evict_one(&self) -> Result<bool> {
        let frames = self.all_frames()?;
        if frames.is_empty() {
            return Ok(false);
        }

        let start = self.clock_hand.fetch_add(1, Ordering::Relaxed);
        // Usage climbs to CLOCK_MAX_USAGE on each pin and only falls here.
        // Two passes cannot decay a hot frame to zero, so a clean unpinned
        // page was reported as unevictable.
        let visits = frames.len().saturating_mul(CLOCK_MAX_USAGE as usize + 1);
        for idx in 0..visits {
            let (page_id, frame) = &frames[(start + idx) % frames.len()];
            let mut state = frame
                .state
                .lock()
                .map_err(|_| Error::CorruptPage("buffer frame poisoned"))?;
            if state.pin_count > 0 || state.write_in_progress || state.page.is_none() {
                continue;
            }
            if state.usage_count > 0 {
                state.usage_count -= 1;
                continue;
            }
            if state.dirty {
                let page = state
                    .page
                    .as_ref()
                    .ok_or(Error::CorruptPage("resident frame missing page"))?;
                let scratch = self.scratch.load(Ordering::Acquire);
                if !scratch && !eviction_may_write(page)? {
                    continue;
                }
                drop(state);
                // The flush rechecks the page LSN under the frame lock, so a
                // page that took a logged change meanwhile stays. Scratch
                // pages have no WAL to wait for.
                self.evicted_unsynced.store(true, Ordering::Release);
                let durable = if scratch { Lsn(u64::MAX) } else { Lsn::ZERO };
                self.flush_frame_if_durable(frame, durable)?;
                state = frame
                    .state
                    .lock()
                    .map_err(|_| Error::CorruptPage("buffer frame poisoned"))?;
                if state.pin_count > 0 || state.dirty || state.write_in_progress {
                    continue;
                }
            }
            drop(state);
            if self.remove_frame_if_unpinned(*page_id, frame)? {
                self.stats.evictions.fetch_add(1, Ordering::Relaxed);
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn remove_loading_frame(&self, page_id: PageId, frame: &Arc<FrameEntry>) -> Result<()> {
        let shard_idx = self.shard_idx(page_id);
        let mut shard = self.shards[shard_idx]
            .lock()
            .map_err(|_| Error::CorruptPage("buffer shard poisoned"))?;
        if shard
            .get(&page_id)
            .map(|resident| Arc::ptr_eq(resident, frame))
            .unwrap_or(false)
        {
            if let Ok(mut state) = frame.state.lock() {
                state.load_failed = true;
            }
            shard.remove(&page_id);
            self.resident.fetch_sub(1, Ordering::Relaxed);
        }
        frame.ready.notify_all();
        Ok(())
    }

    fn remove_frame_if_unpinned(&self, page_id: PageId, frame: &Arc<FrameEntry>) -> Result<bool> {
        let shard_idx = self.shard_idx(page_id);
        let mut shard = self.shards[shard_idx]
            .lock()
            .map_err(|_| Error::CorruptPage("buffer shard poisoned"))?;
        if !shard
            .get(&page_id)
            .map(|resident| Arc::ptr_eq(resident, frame))
            .unwrap_or(false)
        {
            return Ok(false);
        }
        let mut state = frame
            .state
            .lock()
            .map_err(|_| Error::CorruptPage("buffer frame poisoned"))?;
        if state.pin_count > 0 || state.dirty || state.write_in_progress || state.page.is_none() {
            return Ok(false);
        }
        // A pin may already hold this frame from its shard lookup and be
        // waiting for the frame lock. Mark the frame gone so that pin looks
        // the page up again instead of pinning a frame the pool no longer
        // holds, where no flush would ever see its changes.
        state.load_failed = true;
        drop(state);
        shard.remove(&page_id);
        self.resident.fetch_sub(1, Ordering::Relaxed);
        Ok(true)
    }

    fn dirty_frames(&self, durable_lsn: Lsn) -> Result<Vec<Arc<FrameEntry>>> {
        let mut frames = Vec::new();
        for shard in &self.shards {
            let shard = shard
                .lock()
                .map_err(|_| Error::CorruptPage("buffer shard poisoned"))?;
            for frame in shard.values() {
                let state = frame
                    .state
                    .lock()
                    .map_err(|_| Error::CorruptPage("buffer frame poisoned"))?;
                let Some(page) = state.page.as_ref() else {
                    continue;
                };
                if state.dirty && !state.write_in_progress && page.header()?.page_lsn <= durable_lsn
                {
                    frames.push(Arc::clone(frame));
                }
            }
        }
        Ok(frames)
    }

    fn all_frames(&self) -> Result<Vec<(PageId, Arc<FrameEntry>)>> {
        let mut frames = Vec::new();
        for shard in &self.shards {
            let shard = shard
                .lock()
                .map_err(|_| Error::CorruptPage("buffer shard poisoned"))?;
            frames.extend(
                shard
                    .iter()
                    .map(|(page_id, frame)| (*page_id, Arc::clone(frame))),
            );
        }
        crate::observe::add_all_frames(frames.len() as u64);
        Ok(frames)
    }

    fn flush_frame(&self, frame: &Arc<FrameEntry>, durable_lsn: Lsn) -> Result<bool> {
        self.flush_frame_checked(frame, durable_lsn, true)
    }

    fn flush_frame_if_durable(&self, frame: &Arc<FrameEntry>, durable_lsn: Lsn) -> Result<bool> {
        self.flush_frame_checked(frame, durable_lsn, false)
    }

    fn flush_frame_checked(
        &self,
        frame: &Arc<FrameEntry>,
        durable_lsn: Lsn,
        strict: bool,
    ) -> Result<bool> {
        let (page, written_lsn) = {
            let mut state = frame
                .state
                .lock()
                .map_err(|_| Error::CorruptPage("buffer frame poisoned"))?;
            while state.page.is_none() {
                state = frame
                    .ready
                    .wait(state)
                    .map_err(|_| Error::CorruptPage("buffer frame wait poisoned"))?;
            }
            if !state.dirty {
                return Ok(false);
            }
            let page = state
                .page
                .as_ref()
                .ok_or(Error::CorruptPage("resident frame missing page"))?;
            let page_lsn = page.header()?.page_lsn;
            if page_lsn > durable_lsn {
                return if strict {
                    Err(Error::CorruptPage("dirty page lsn exceeds durable wal lsn"))
                } else {
                    Ok(false)
                };
            }
            let page = page.clone();
            state.write_in_progress = true;
            (page, page_lsn)
        };

        #[cfg(test)]
        super::buffer_test_hooks::run_before_page_write_hook(page.header()?.page_id, written_lsn);
        let write_result = self.page_file.write_page(&page);
        let mut state = frame
            .state
            .lock()
            .map_err(|_| Error::CorruptPage("buffer frame poisoned"))?;
        state.write_in_progress = false;
        match write_result {
            Ok(()) => {
                let current_lsn = state
                    .page
                    .as_ref()
                    .ok_or(Error::CorruptPage("resident frame missing page"))?
                    .header()?
                    .page_lsn;
                if current_lsn <= written_lsn {
                    state.dirty = false;
                }
                self.stats.writes.fetch_add(1, Ordering::Relaxed);
                frame.ready.notify_all();
                Ok(true)
            }
            Err(err) => {
                frame.ready.notify_all();
                Err(err)
            }
        }
    }

    fn shard_idx(&self, page_id: PageId) -> usize {
        page_id.0 as usize % self.shards.len()
    }
}

/// Whether eviction may write this dirty page out on its own.
///
/// Only a page no logged change has touched since it was allocated, read or
/// replayed, which is a page with LSN zero. Any other dirty page reaches the
/// file only through a checkpoint, which writes every such page as one cut
/// and records where recovery starts. Recovery replays heap records into
/// new versions rather than onto the page they changed, so a heap page
/// written ahead of that cut leaves a second copy of every row on it once
/// the records are replayed again. Undo records are never logged, so a heap
/// page written ahead of the undo page its new versions point into can name
/// undo the file never received. A B-tree split changes several pages, and
/// HNSW links in pages it never logs, so one of those pages alone can point
/// at a page the file does not have. When every unpinned frame holds such a
/// page, eviction asks the engine for a checkpoint if the engine enabled
/// that, and otherwise fails the allocation.
fn eviction_may_write(page: &Page) -> Result<bool> {
    Ok(page.header()?.page_lsn == Lsn::ZERO)
}

impl PageGuard {
    pub fn page_id(&self) -> PageId {
        self.page_id
    }

    pub fn with_page<R>(&self, f: impl FnOnce(&Page) -> Result<R>) -> Result<R> {
        let frame = self.frame()?;
        let page = frame
            .page
            .as_ref()
            .ok_or(Error::CorruptPage("resident frame missing page"))?;
        f(page)
    }

    pub fn with_page_mut<R>(&self, f: impl FnOnce(&mut Page) -> Result<R>) -> Result<R> {
        let mut frame = self.mutable_frame()?;
        let page = frame
            .page
            .as_mut()
            .ok_or(Error::CorruptPage("resident frame missing page"))?;
        f(page)
    }

    pub fn mark_dirty(&self, lsn: Lsn) -> Result<()> {
        let mut frame = self.mutable_frame()?;
        let page = frame
            .page
            .as_mut()
            .ok_or(Error::CorruptPage("resident frame missing page"))?;
        page.set_page_lsn(lsn)?;
        frame.dirty = true;
        Ok(())
    }

    /// Replace the resident page and mark it dirty while holding the frame
    /// lock. Callers stage bytes privately, append the WAL record, then
    /// publish here so a flush cannot observe the new bytes under the
    /// previous page LSN.
    pub(crate) fn install_dirty(&self, new_page: Page, lsn: Lsn) -> Result<()> {
        let mut frame = self.mutable_frame()?;
        let page = frame
            .page
            .as_mut()
            .ok_or(Error::CorruptPage("resident frame missing page"))?;
        *page = new_page;
        page.set_page_lsn(lsn)?;
        frame.dirty = true;
        Ok(())
    }

    fn frame(&self) -> Result<MutexGuard<'_, FrameState>> {
        self.frame
            .state
            .lock()
            .map_err(|_| Error::CorruptPage("buffer frame poisoned"))
    }

    pub(crate) fn mutable_frame(&self) -> Result<MutexGuard<'_, FrameState>> {
        let mut frame = self.frame()?;
        while frame.write_in_progress {
            frame = self
                .frame
                .ready
                .wait(frame)
                .map_err(|_| Error::CorruptPage("buffer frame wait poisoned"))?;
        }
        Ok(frame)
    }
}

impl Drop for PageGuard {
    fn drop(&mut self) {
        if let Ok(mut frame) = self.frame() {
            frame.pin_count = frame.pin_count.saturating_sub(1);
            self.frame.ready.notify_all();
        }
    }
}
