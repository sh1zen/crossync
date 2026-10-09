use crate::core::futex::{Futex, futex_wait, futex_wake, futex_wake_all};
use crate::core::smutex::SGuard;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

pub struct SCondVar {
    futex: Futex, // For futex_wait/wake
    pub(crate) waiters: AtomicUsize,
    pub(crate) to_wake: AtomicUsize,
}

impl SCondVar {
    pub const fn new() -> Self {
        Self {
            futex: Futex::new(0),
            waiters: AtomicUsize::new(0),
            to_wake: AtomicUsize::new(0),
        }
    }

    pub fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if condition() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::yield_now();
        }
    }

    pub fn wait_for_waiters(&self, count: usize, timeout: Duration) -> bool {
        Self::wait_until(timeout, || self.waiters.load(Ordering::Acquire) >= count)
    }

    pub(crate) fn wait<'a>(&self, guard: SGuard<'a>) -> SGuard<'a> {
        let mutex = guard.m;
        let is_group = guard.is_group;
        std::mem::forget(guard);

        // Register as waiter
        self.waiters.fetch_add(1, Ordering::SeqCst);

        if is_group {
            mutex.raw_unlock_group();
        } else {
            mutex.raw_unlock();
        }

        loop {
            let futex_val = self.futex.load(Ordering::Acquire);

            // Try to consume a wake token
            let mut to_wake = self.to_wake.load(Ordering::Acquire);
            while to_wake > 0 {
                match self.to_wake.compare_exchange_weak(
                    to_wake,
                    to_wake - 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        // Successfully consumed a token, we can wake
                        self.waiters.fetch_sub(1, Ordering::SeqCst);
                        return if is_group {
                            mutex.lock_group()
                        } else {
                            mutex.lock()
                        };
                    }
                    Err(new_val) => to_wake = new_val,
                }
            }

            // No token available, wait
            futex_wait(&self.futex, futex_val);
        }
    }

    pub fn notify_one(&self) {
        if self.waiters.load(Ordering::SeqCst) > 0 {
            self.to_wake.fetch_add(1, Ordering::Release);
            self.futex.fetch_add(1, Ordering::Release);
            futex_wake(&self.futex);
        }
    }

    pub fn notify_all(&self) {
        let waiters = self.waiters.load(Ordering::SeqCst);
        if waiters > 0 {
            self.to_wake.fetch_add(waiters, Ordering::Release);
            self.futex.fetch_add(1, Ordering::Release);
            futex_wake_all(&self.futex);
        }
    }
}
