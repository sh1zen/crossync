use crate::core::futex::{Futex, futex_wait, futex_wake_all};
use crate::sync::Backoff;
use std::cell::Cell;
use std::fmt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const COUNT: u64 = (1 << 30) - 1;
const WRITE: u64 = 1 << 30;
const WAIT: u64 = 1 << 31;

/// Read aliases stay on their acquiring thread; writers never alias readers.
/// Read guards may only leave that thread when the guarded T is Sync.
/// Ownership, count and waiter registration share one CAS word so a last
/// foreign guard drop cannot race a reentrant acquisition or lose a wakeup.
pub(crate) struct SerialMutex {
    state: AtomicU64,
    wake: Futex,
}
pub(crate) struct SerialGuard<'a> {
    lock: &'a SerialMutex,
    owner: u32,
    read: bool,
}

#[inline]
fn thread_owner() -> u64 {
    thread_local! {
        static OWNER: Cell<u64> = const { Cell::new(0) };
    }
    OWNER.with(|owner| {
        let id = owner.get();
        if id != 0 { id } else { initialize_owner(owner) }
    })
}

#[cold]
fn initialize_owner(owner: &Cell<u64>) -> u64 {
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    // Never reuse an owner id, including after thread termination.
    if id >= u32::MAX as usize {
        std::process::abort();
    }
    let id = (id as u64) << 32;
    owner.set(id);
    id
}

impl SerialMutex {
    pub(crate) const fn new() -> Self {
        Self {
            state: AtomicU64::new(0),
            wake: Futex::new(0),
        }
    }
    #[inline]
    pub(crate) fn read_guard(&self) -> SerialGuard<'_> {
        let owner = self.lock_read();
        SerialGuard {
            lock: self,
            owner,
            read: true,
        }
    }
    #[inline]
    pub(crate) fn write_guard(&self) -> SerialGuard<'_> {
        self.lock_write();
        SerialGuard {
            lock: self,
            owner: 0,
            read: false,
        }
    }
    pub(crate) fn try_write_guard(&self) -> Option<SerialGuard<'_>> {
        let desired = thread_owner() | WRITE;
        if self
            .state
            .compare_exchange(0, desired, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(SerialGuard {
                lock: self,
                owner: 0,
                read: false,
            })
        } else {
            None
        }
    }
    #[inline]
    pub(crate) fn lock_read(&self) -> u32 {
        self.lock(true)
    }
    #[inline]
    pub(crate) fn lock_write(&self) {
        self.lock(false);
    }
    #[inline]
    fn lock(&self, read: bool) -> u32 {
        let owner = thread_owner();
        let desired = owner | if read { 1 } else { WRITE };
        if self
            .state
            .compare_exchange(0, desired, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            return (owner >> 32) as u32;
        }
        self.lock_slow(owner, read, desired);
        (owner >> 32) as u32
    }
    #[cold]
    fn lock_slow(&self, owner: u64, read: bool, desired: u64) {
        let backoff = Backoff::new();
        loop {
            // Snapshot before waiter registration, so an intervening unlock
            // either invalidates the state CAS or changes the futex sequence.
            let sequence = self.wake.load(Ordering::Acquire);
            let state = self.state.load(Ordering::Relaxed);
            let next = if state == 0 {
                desired
            } else if state & !(u32::MAX as u64) == owner {
                assert!(
                    read && state & WRITE == 0,
                    "conflicting reentrant Atomic access"
                );
                assert!(state & COUNT != COUNT, "Atomic reader count overflow");
                state + 1
            } else {
                if !backoff.is_completed() {
                    backoff.snooze();
                    continue;
                }
                let parked = state | WAIT;
                if self
                    .state
                    .compare_exchange(state, parked, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
                {
                    futex_wait(&self.wake, sequence);
                    backoff.reset();
                }
                continue;
            };
            if self
                .state
                .compare_exchange_weak(state, next, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
        }
    }
    #[inline]
    pub(crate) fn unlock_read(&self, owner: u32) {
        // The acquisition token gives a cheap uncontended last-reader CAS.
        // A registered waiter or another reader changes this exact word and
        // falls back to the counted release, preserving all waiter flags.
        let expected = ((owner as u64) << 32) | 1;
        let mut state =
            match self
                .state
                .compare_exchange(expected, 0, Ordering::Release, Ordering::Relaxed)
            {
                Ok(_) => return,
                Err(state) => state,
            };
        loop {
            debug_assert!(state & WRITE == 0 && state & COUNT != 0);
            let last = state & COUNT == 1;
            let next = if last { 0 } else { state - 1 };
            match self.state.compare_exchange_weak(
                state,
                next,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    if last && state & WAIT != 0 {
                        self.notify();
                    }
                    return;
                }
                Err(current) => state = current,
            }
        }
    }
    #[inline]
    pub(crate) fn unlock_write(&self) {
        let state = self.state.swap(0, Ordering::Release);
        debug_assert!(state & WRITE != 0);
        if state & WAIT != 0 {
            self.notify();
        }
    }
    #[inline]
    pub(crate) fn is_read_locked(&self) -> bool {
        self.state.load(Ordering::Relaxed) & COUNT != 0
    }
    #[inline]
    pub(crate) fn is_write_locked(&self) -> bool {
        self.state.load(Ordering::Relaxed) & WRITE != 0
    }
    pub(crate) fn owned_by_current_thread(&self) -> bool {
        self.state.load(Ordering::Relaxed) & !(u32::MAX as u64) == thread_owner()
    }
    fn notify(&self) {
        self.wake.fetch_add(1, Ordering::Release);
        futex_wake_all(&self.wake);
    }
}
impl SerialGuard<'_> {
    pub(crate) fn owner_token(&self) -> u32 {
        self.owner
    }
}
impl Drop for SerialGuard<'_> {
    #[inline]
    fn drop(&mut self) {
        if self.read {
            self.lock.unlock_read(self.owner);
        } else {
            self.lock.unlock_write();
        }
    }
}
impl fmt::Debug for SerialMutex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SerialMutex")
            .field("state", &self.state.load(Ordering::Relaxed))
            .finish()
    }
}
