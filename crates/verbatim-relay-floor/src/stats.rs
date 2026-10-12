//! The figures a run reports: each hop's times, and the heap allocations a
//! process makes while it handles frames.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

/// The system allocator, counting every allocation the process makes.
pub struct Counting;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

// SAFETY: every call is passed on unchanged to the system allocator, which
// upholds `GlobalAlloc`'s contract; the count is only a side effect.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the caller's guarantees for `alloc`, passed on.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the caller's guarantees for `alloc_zeroed`, passed on.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's guarantees for `dealloc`, passed on.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the caller's guarantees for `realloc`, passed on.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// How many allocations (and reallocations) the process has made so far.
pub fn allocations() -> u64 {
    ALLOCATIONS.load(Ordering::Relaxed)
}

/// Allocations per frame, for a report.
pub fn per_frame(allocations: u64, frames: u64) -> String {
    if frames == 0 {
        return "no frames".to_owned();
    }
    #[expect(clippy::cast_precision_loss, reason = "counts far below 2^52")]
    let ratio = allocations as f64 / frames as f64;
    format!("{ratio:.2} per frame ({allocations} for {frames} frames)")
}

/// Writes that pended, for a report.
pub fn pended(writes: u64, pended: u64) -> String {
    if writes == 0 {
        return "not overlapped".to_owned();
    }
    format!("{pended} of {writes} writes pended")
}

/// Times in microseconds, for one hop.
pub struct Samples {
    name: &'static str,
    us: Vec<f64>,
}

impl Samples {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            us: Vec::new(),
        }
    }

    /// Adds the span from `from` to `to`, in units of 100 ns; a clock that
    /// ran backwards counts as zero.
    pub fn add(&mut self, from: u64, to: u64) {
        #[expect(clippy::cast_precision_loss, reason = "spans far below 2^52")]
        let us = to.saturating_sub(from) as f64 / 10.0;
        self.us.push(us);
    }

    /// One line: the count, the percentiles, the maximum, and how many were
    /// over 0.5, 1 and 2 ms, with the share over 1 and 2 ms.
    pub fn report(&mut self) -> String {
        self.us.sort_by(f64::total_cmp);
        let count = self.us.len();
        if count == 0 {
            return format!("{}: no samples", self.name);
        }
        #[expect(clippy::cast_precision_loss, reason = "a count of samples")]
        let total = count as f64;
        let at = |fraction: f64| {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a rank within the samples"
            )]
            let rank = ((fraction * total).ceil() as usize).clamp(1, count);
            self.us[rank - 1] / 1_000.0
        };
        let over = |ms: f64| self.us.iter().filter(|&&us| us > ms * 1_000.0).count();
        #[expect(clippy::cast_precision_loss, reason = "a count of samples")]
        let share = |ms: f64| over(ms) as f64 * 100.0 / total;
        format!(
            "{}: {count} samples; median {:.3} ms, 99th {:.3} ms, 99.9th {:.3} ms, \
             99.99th {:.3} ms, maximum {:.3} ms; over 0.5 ms {}, over 1 ms {} ({:.2} percent), \
             over 2 ms {} ({:.2} percent)",
            self.name,
            at(0.5),
            at(0.99),
            at(0.999),
            at(0.9999),
            self.us[count - 1] / 1_000.0,
            over(0.5),
            over(1.0),
            share(1.0),
            over(2.0),
            share(2.0),
        )
    }
}
