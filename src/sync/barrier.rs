use crate::core::scondvar::SCondVar;
use crate::core::smutex::SMutex;
use std::sync::atomic;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Internal representation of the barrier.
/// - `ref_count`: Tracks the number of clones to manage cleanup.
/// - `waiters`: Number of threads currently required to reach the barrier.
/// - `bucket`: Number of waiters to reset the barrier after a group completes (if > 0).
/// - `sync` and `cond`: Used for blocking threads.
struct BarrierInner {
    ref_count: AtomicUsize,
    waiters: AtomicUsize,
    bucket: usize,
    mutex: SMutex,
    cond: SCondVar,
}

/// A reusable synchronization barrier.
///
/// Multiple threads can wait on the barrier until a threshold is reached.
/// After the required number of threads have called `wait()`, they are released.
#[repr(transparent)]
pub struct Barrier {
    ptr: *const BarrierInner,
}

// Barrier is safe to send and share across threads
unsafe impl Send for Barrier {}
unsafe impl Sync for Barrier {}

impl Barrier {
    /// Creates a basic barrier with a single waiter.
    /// This results in a non-blocking barrier (useful as a no-op).
    pub fn new() -> Barrier {
        Self::init(1, 0)
    }

    /// Creates a barrier with a given number of required waiters (`n`) and
    /// a reusable capacity (`bucket`) for resetting the barrier.
    ///
    /// - When `n` threads reach the barrier, the barrier resets to `bucket`.
    /// - If `bucket == 0`, the barrier will be disabled after use.
    pub fn with_capacity(n: usize, bucket: usize) -> Barrier {
        Self::init(n + 2, if bucket == 0 { 0 } else { bucket + 2 })
    }

    /// Internal initializer for the barrier.
    fn init(n: usize, bucket: usize) -> Barrier {
        let ptr = Box::into_raw(Box::new(BarrierInner {
            ref_count: AtomicUsize::new(1),
            waiters: AtomicUsize::new(n),
            bucket,
            mutex: SMutex::new(),
            cond: SCondVar::new(),
        }));

        if ptr.is_null() {
            panic!("Invalid allocation for Barrier");
        }

        Self { ptr }
    }

    /// Access the internal `BarrierInner` safely.
    #[inline(always)]
    fn inner(&self) -> &BarrierInner {
        unsafe { &*self.ptr }
    }

    /// Returns the current number of threads needed to trigger the barrier.
    pub fn count(&self) -> usize {
        self.inner().waiters.load(Ordering::Acquire)
    }

    /// Waits at the barrier until the required number of threads arrive.
    pub fn wait(&self) {
        let inner = self.inner();

        // Read current waiters count
        let waiters = inner.waiters.load(Ordering::Acquire);

        // No-op if barrier is disabled or only one reference exists
        if waiters == 0 || inner.ref_count.load(Ordering::Acquire) == 1 {
            return;
        }

        // Lock sync for coordination
        let guard = inner.mutex.lock();

        if waiters > 1 {
            // Decrement number of waiters
            let new_val = inner.waiters.fetch_sub(1, Ordering::AcqRel) - 1;

            if new_val == 2 {
                // Second-to-last thread: reset the barrier and notify all
                inner.waiters.store(inner.bucket, Ordering::Release);
                self.release();
            } else {
                // Wait until condition variable is notified
                let _ = inner.cond.wait(guard);
            }
        } else {
            // Last thread: wait for notification
            let _ = inner.cond.wait(guard);
        }
    }

    /// Releases all waiting threads.
    /// Typically called automatically by the last thread, but can be called manually.
    #[inline(always)]
    pub fn release(&self) {
        self.inner().cond.notify_all();
    }
}

// Implement clone so that the barrier can be shared safely across threads
impl Clone for Barrier {
    fn clone(&self) -> Self {
        self.inner().ref_count.fetch_add(1, Ordering::Release);
        Barrier { ptr: self.ptr }
    }
}

// Drop implementation for safe cleanup once all references are released
impl Drop for Barrier {
    fn drop(&mut self) {
        if self.inner().ref_count.fetch_sub(1, Ordering::Release) == 1 {
            atomic::fence(Ordering::Acquire); // Ensure all writes are visible
            let ptr = self.ptr as *mut BarrierInner;
            unsafe { drop(Box::from_raw(ptr)) };
        }
    }
}

// Support Default trait for `Barrier::default()`
impl Default for Barrier {
    fn default() -> Self {
        Self::new()
    }
}
