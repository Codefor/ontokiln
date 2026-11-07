//! Only query workers enforce a heap limit. Exit before the allocator can
//! exhaust the host, even inside an eager third-party join or sort.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
};

pub const MEMORY_EXIT: i32 = 75;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static TOTAL: AtomicUsize = AtomicUsize::new(0);
static LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);
static ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub struct Allocator;
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;
fn reserve(n: usize) {
    let live = LIVE.fetch_add(n, Relaxed) + n;
    if !ACTIVE.load(Relaxed) {
        return;
    }
    if live > LIMIT.load(Relaxed) {
        unsafe { libc::_exit(MEMORY_EXIT) }
    }
    PEAK.fetch_max(live, Relaxed);
    TOTAL.fetch_add(n, Relaxed);
}
// Tracking begins in the synchronous, single-thread worker before reading
// any query-owned data. Allocations predating activation remain live until exit.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        reserve(layout.size());
        let ptr = unsafe { System.alloc(layout) };
        if ptr.is_null() {
            LIVE.fetch_sub(layout.size(), Relaxed);
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, old: Layout, n: usize) -> *mut u8 {
        if n >= old.size() {
            reserve(n - old.size());
        } else {
            LIVE.fetch_sub(old.size() - n, Relaxed);
        }
        let ptr = unsafe { System.realloc(ptr, old, n) };
        if ptr.is_null() {
            if n >= old.size() {
                LIVE.fetch_sub(n - old.size(), Relaxed);
            } else {
                LIVE.fetch_add(old.size() - n, Relaxed);
            }
        }
        ptr
    }
}
static BASE: AtomicUsize = AtomicUsize::new(0);
pub fn begin(limit: usize) {
    let base = LIVE.load(Relaxed);
    BASE.store(base, Relaxed);
    PEAK.store(base, Relaxed);
    LIMIT.store(base.saturating_add(limit), Relaxed);
    ACTIVE.store(true, Relaxed);
}
pub fn peak() -> usize {
    PEAK.load(Relaxed).saturating_sub(BASE.load(Relaxed))
}
pub fn meter() -> (u64, u64) {
    (
        ontokiln_reasoner::sparql::profile_visits(),
        TOTAL.load(Relaxed) as u64,
    )
}
