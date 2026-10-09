pub mod backoff;
pub(crate) mod coord;
pub(crate) mod serial;
pub(crate) mod futex;
pub(crate) mod mutex;
pub mod scondvar;
pub(crate) mod smutex;
pub(crate) mod thread;

/// Bound forgotten-clone counts before overflow can free live storage.
#[inline]
pub(crate) fn increment_ref_count(count: &std::sync::atomic::AtomicUsize) {
    if count.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= isize::MAX as usize {
        std::process::abort();
    }
}
