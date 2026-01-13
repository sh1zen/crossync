use crate::core::futex::{futex_wait, futex_wake};
use crate::sync::Backoff;
use crossbeam_utils::CachePadded;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

/// Internal bit flags
const UNLOCKED: usize = 0;
const LOCKED: usize = 1;
const GROUP_FLAG: usize = 2;

/// Recursive futex-based mutex supporting exclusive and group (shared) locks
pub(crate) struct SMutex {
    state: CachePadded<AtomicUsize>, // LOCKED/GROUP_FLAG
    pub(crate) owner: CachePadded<AtomicUsize>, // thread ID for recursion
    pub(crate) recursion: CachePadded<AtomicUsize>, // recursion count
}

impl SMutex {
    pub(crate) fn new() -> Self {
        Self {
            state: CachePadded::new(AtomicUsize::new(UNLOCKED)),
            owner: CachePadded::new(AtomicUsize::new(0)),
            recursion: CachePadded::new(AtomicUsize::new(0)),
        }
    }

    fn thread_id() -> usize {
        // Convert ThreadId to usize
        let tid: thread::ThreadId = thread::current().id();
        unsafe { std::mem::transmute::<thread::ThreadId, usize>(tid) }
    }

    /// Exclusive lock
    pub(crate) fn lock(&self) -> SGuard<'_> {
        let tid = Self::thread_id();

        // Fast path: already owner → increment recursion
        if self.owner.load(Ordering::Relaxed) == tid {
            self.recursion.fetch_add(1, Ordering::Relaxed);
            return SGuard::new(self);
        }

        // Try acquire
        let spin = Backoff::new();
        loop {
            if self
                .state
                .compare_exchange_weak(UNLOCKED, LOCKED, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                self.owner.store(tid, Ordering::Relaxed);
                self.recursion.store(1, Ordering::Relaxed);
                return SGuard::new(self);
            }

            if !spin.is_yielding() {
                spin.snooze();
                continue;
            }

            while self.state.load(Ordering::Relaxed) & LOCKED != 0 {
                futex_wait(&self.state, LOCKED);
            }
        }
    }

    /// Group/shared lock
    pub(crate) fn lock_group(&self) -> SGuard<'_> {
        let tid = Self::thread_id();

        // Already exclusive owner → can reenter
        if self.owner.load(Ordering::Relaxed) == tid {
            self.recursion.fetch_add(1, Ordering::Relaxed);
            return SGuard::new_group(self);
        }

        let spin = Backoff::new();
        loop {
            let mut state = self.state.load(Ordering::Relaxed);

            while state & LOCKED == 0 {
                let new_state = state | GROUP_FLAG;
                match self.state.compare_exchange_weak(
                    state,
                    new_state,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return SGuard::new_group(self),
                    Err(e) => state = e,
                }
            }

            if state & LOCKED != 0 {
                spin.snooze();
                while self.state.load(Ordering::Relaxed) & LOCKED != 0 {
                    futex_wait(&self.state, state);
                }
            }
        }
    }

    /// Unlock exclusive lock
    pub(crate) fn raw_unlock(&self) {
        let tid = Self::thread_id();
        if self.owner.load(Ordering::Relaxed) != tid {
            panic!("Unlock called by non-owner thread");
        }

        let rec = self.recursion.fetch_sub(1, Ordering::Relaxed);
        if rec > 1 {
            return; // still holds recursion
        }

        // fully release
        self.owner.store(0, Ordering::Relaxed);
        self.state.fetch_and(!LOCKED, Ordering::Release);
        futex_wake(&*self.state);
    }

    /// Unlock group lock
    pub(crate) fn raw_unlock_group(&self) {
        let tid = Self::thread_id();
        if self.owner.load(Ordering::Relaxed) == tid {
            // recursion path
            let rec = self.recursion.fetch_sub(1, Ordering::Relaxed);
            if rec > 1 {
                return;
            }
            self.owner.store(0, Ordering::Relaxed);
        }

        self.state.fetch_and(!GROUP_FLAG, Ordering::Release);
        futex_wake(&*self.state);
    }

    pub(crate) fn is_locked(&self) -> bool {
        self.state.load(Ordering::Relaxed) & LOCKED != 0
    }
}

/// RAII guard
pub(crate) struct SGuard<'a> {
    pub(crate) m: &'a SMutex,
    pub(crate) is_group: bool,
}

impl<'a> SGuard<'a> {
    fn new(m: &'a SMutex) -> Self {
        Self { m, is_group: false }
    }

    pub(crate) fn new_group(m: &'a SMutex) -> Self {
        Self { m, is_group: true }
    }

    /// Explicit unlock
    pub(crate) fn unlock(this: &SGuard<'_>) {
        if this.is_group {
            this.m.raw_unlock_group();
        } else {
            this.m.raw_unlock();
        }
    }

    /// Explicit lock (reacquire)
    pub(crate) fn lock(this: &SGuard<'_>) {
        if this.is_group {
            this.m.lock_group();
        } else {
            this.m.lock();
        }
    }
}

impl<'a> Drop for SGuard<'a> {
    fn drop(&mut self) {
        if self.is_group {
            self.m.raw_unlock_group();
        } else {
            self.m.raw_unlock();
        }
    }
}
