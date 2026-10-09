use crate::core::futex::{Futex, futex_wait, futex_wake};
use crate::sync::Backoff;
use crossbeam_utils::CachePadded;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Internal bit flags
const UNLOCKED: u32 = 0;
const LOCKED: u32 = 1;
const ONE_GROUP: u32 = 2;

/// Recursive futex-based mutex supporting exclusive and group (shared) locks
pub(crate) struct SMutex {
    state: CachePadded<Futex>,                  // LOCKED or shared-holder count
    pub(crate) owner: CachePadded<AtomicUsize>, // thread ID for recursion
    pub(crate) recursion: CachePadded<AtomicUsize>, // recursion count
}

impl SMutex {
    pub(crate) fn new() -> Self {
        Self {
            state: CachePadded::new(Futex::new(UNLOCKED)),
            owner: CachePadded::new(AtomicUsize::new(0)),
            recursion: CachePadded::new(AtomicUsize::new(0)),
        }
    }

    fn thread_id() -> usize {
        thread_local! {
            static ID: usize = {
                static NEXT: AtomicUsize = AtomicUsize::new(1);
                NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                    .expect("thread identifier overflow")
            };
        }
        ID.with(|id| *id)
    }

    /// Exclusive lock
    pub(crate) fn lock(&self) -> SGuard<'_> {
        let tid = Self::thread_id();

        // Fast path: already owner -> increment recursion
        if self.owner.load(Ordering::Relaxed) == tid {
            self.recursion.fetch_add(1, Ordering::Relaxed);
            return SGuard::new(self);
        }

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

            let mut state = self.state.load(Ordering::Relaxed);
            while state != UNLOCKED {
                futex_wait(&self.state, state);
                state = self.state.load(Ordering::Relaxed);
            }
        }
    }

    /// Group/shared lock
    pub(crate) fn lock_group(&self) -> SGuard<'_> {
        let tid = Self::thread_id();

        // Already exclusive owner -> can reenter
        if self.owner.load(Ordering::Relaxed) == tid {
            self.recursion.fetch_add(1, Ordering::Relaxed);
            return SGuard::new_group(self);
        }

        let spin = Backoff::new();
        loop {
            let mut state = self.state.load(Ordering::Relaxed);

            while state & LOCKED == 0 {
                let new_state = state
                    .checked_add(ONE_GROUP)
                    .expect("SMutex group count overflow");

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

            spin.snooze();
            while self.state.load(Ordering::Relaxed) & LOCKED != 0 {
                futex_wait(&self.state, LOCKED);
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

        self.owner.store(0, Ordering::Relaxed);
        self.state.store(UNLOCKED, Ordering::Release);
        futex_wake(&*self.state);
    }

    /// Unlock group lock
    pub(crate) fn raw_unlock_group(&self) {
        let tid = Self::thread_id();
        if self.owner.load(Ordering::Relaxed) == tid {
            let rec = self.recursion.fetch_sub(1, Ordering::Relaxed);
            if rec > 1 {
                return;
            }

            self.owner.store(0, Ordering::Relaxed);
            self.state.store(UNLOCKED, Ordering::Release);
            futex_wake(&*self.state);
            return;
        }

        let prev = self.state.fetch_sub(ONE_GROUP, Ordering::Release);
        debug_assert!(
            prev >= ONE_GROUP,
            "unlock_group without a matching lock_group"
        );

        if prev == ONE_GROUP {
            futex_wake(&*self.state);
        }
    }

    pub(crate) fn is_locked(&self) -> bool {
        self.state.load(Ordering::Relaxed) & LOCKED != 0
    }
}

/// RAII guard
pub(crate) struct SGuard<'a> {
    pub(crate) m: &'a SMutex,
    pub(crate) is_group: bool,
    not_send: PhantomData<Rc<()>>,
}

impl<'a> SGuard<'a> {
    fn new(m: &'a SMutex) -> Self {
        Self {
            m,
            is_group: false,
            not_send: PhantomData,
        }
    }

    pub(crate) fn new_group(m: &'a SMutex) -> Self {
        Self {
            m,
            is_group: true,
            not_send: PhantomData,
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
