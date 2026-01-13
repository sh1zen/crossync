use crate::core::futex::{futex_wait, futex_wake, futex_wake_all};
use crate::core::smutex::SGuard;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(crate) struct SCondVar {
    pub(crate) epoch: AtomicUsize,
}

impl SCondVar {
    #[inline]
    pub(crate) const fn new() -> Self {
        Self {
            epoch: AtomicUsize::new(0),
        }
    }

    /// Wait on the condition variable.
    ///
    /// This atomically unlocks the mutex, waits for a notification,
    /// and re-locks the mutex before returning.
    pub(crate) fn wait<'a>(&self, guard: SGuard<'a>) -> SGuard<'a> {
        // Capture guard info before forgetting it
        let mutex = guard.m;
        let is_group = guard.is_group;

        // Prevent double-unlock: forget the guard so Drop doesn't run
        std::mem::forget(guard);

        loop {
            let current_epoch = self.epoch.load(Ordering::Acquire);

            // Unlock the mutex before sleeping
            if is_group {
                mutex.raw_unlock_group();
            } else {
                mutex.raw_unlock();
            }

            // Wait for notification (spurious wakeups possible)
            futex_wait(&self.epoch, current_epoch);

            // Re-lock the mutex
            let new_guard = if is_group {
                mutex.lock_group()
            } else {
                mutex.lock()
            };

            // Check if epoch changed (real notification vs spurious wakeup)
            if self.epoch.load(Ordering::Acquire) != current_epoch {
                return new_guard;
            }

            // Spurious wakeup - forget guard and retry
            std::mem::forget(new_guard);
        }
    }

    /// Wake one waiting thread.
    pub(crate) fn notify_one(&self) {
        self.epoch.fetch_add(1, Ordering::Release);
        futex_wake(&self.epoch);
    }

    /// Wake all waiting threads.
    pub(crate) fn notify_all(&self) {
        self.epoch.fetch_add(1, Ordering::Release);
        futex_wake_all(&self.epoch);
    }
}
