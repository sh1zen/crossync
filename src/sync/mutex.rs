use crate::core::futex::{futex_wait, futex_wake, futex_wake_all};
use crate::sync::Backoff;
use crossbeam_utils::CachePadded;
use std::fmt;
use std::sync::atomic;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Bitmask-based sync state representation.
type State = usize;

/// Mutex state flags (bitwise layout):
///
/// - UNLOCKED (0): Mutex is completely unlocked
/// - READERS_PARKED: There are readers waiting
/// - WRITERS_PARKED: There are writers waiting
/// - ONE_READER (0b0100): Reader count starts at bit 2 (<< 2)
/// - ONE_WRITER: Special case where all bits but parked flags are set,
///   uniquely identifies writer lock.
const UNLOCKED: State = 0;
const READERS_PARKED: State = 0b0001;
const WRITERS_PARKED: State = 0b0010;
const ONE_READER: State = 0b0100;
const ONE_WRITER: State = !(READERS_PARKED | WRITERS_PARKED);

/// Returns true if the state represents an exclusive writer lock
#[inline]
fn is_writer_locked(state: State) -> bool {
    (state & ONE_WRITER) == ONE_WRITER
}

/// Extracts the number of active readers from the state bits
#[inline]
fn readers_count(state: State) -> usize {
    if is_writer_locked(state) {
        0
    } else {
        (state & ONE_WRITER) >> 2
    }
}

/// The internal sync structure, reference-counted and futex-aware
struct InnerMutex {
    /// Atomic sync state (bitfield)
    state: CachePadded<AtomicUsize>,

    /// Futex sequence number used for reader wait/wake
    readers_futex: CachePadded<AtomicUsize>,

    /// Futex sequence number used for writer wait/wake
    writers_futex: CachePadded<AtomicUsize>,

    /// Reference counter (atomic), for safe cloning and deallocation
    ref_count: CachePadded<AtomicUsize>,
}

impl InnerMutex {
    /// Creates a new `InnerMutex` with all fields initialized.
    fn new() -> Self {
        Self {
            state: CachePadded::new(AtomicUsize::new(0)),
            ref_count: CachePadded::new(AtomicUsize::new(1)),
            readers_futex: CachePadded::new(AtomicUsize::new(0)),
            writers_futex: CachePadded::new(AtomicUsize::new(0)),
        }
    }
}

/// Public sync interface
#[repr(transparent)]
pub struct Mutex {
    ptr: *const InnerMutex,
}

// The sync can be safely sent across threads and shared concurrently
unsafe impl Send for Mutex {}
unsafe impl Sync for Mutex {}

impl std::panic::UnwindSafe for Mutex {}
impl std::panic::RefUnwindSafe for Mutex {}

impl Mutex {
    /// Creates a new `Mutex` instance (initial ref_count = 1)
    pub fn new() -> Self {
        let ptr = Box::into_raw(Box::new(InnerMutex::new()));
        Self { ptr }
    }

    /// Returns a shared reference to the internal sync data
    #[inline(always)]
    fn inner(&self) -> &InnerMutex {
        unsafe { &*self.ptr }
    }

    /// Returns the current reference count
    pub fn get_ref_count(&self) -> usize {
        self.inner().ref_count.load(Ordering::Acquire)
    }

    /// Returns the number of active shared locks (readers)
    pub fn get_shared_locked(&self) -> usize {
        readers_count(self.inner().state.load(Ordering::Acquire))
    }

    /// Returns true if the sync is locked in exclusive (writer) mode
    #[inline]
    pub fn is_locked_exclusive(&self) -> bool {
        is_writer_locked(self.inner().state.load(Ordering::Acquire))
    }

    /// Returns true if there are active readers holding the lock
    #[inline]
    pub fn is_locked_shared(&self) -> bool {
        let s = self.inner().state.load(Ordering::Acquire);
        !is_writer_locked(s) && readers_count(s) > 0
    }

    /// Returns true if the sync is locked in any mode
    pub fn is_locked(&self) -> bool {
        self.inner().state.load(Ordering::Acquire) != UNLOCKED
    }

    /// Attempts to acquire an exclusive (writer) lock without blocking
    #[inline]
    pub fn try_lock_exclusive(&self) -> bool {
        self.inner()
            .state
            .compare_exchange(UNLOCKED, ONE_WRITER, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Acquires the exclusive (writer) lock, blocking if necessary
    pub fn lock_exclusive(&self) {
        if self
            .inner()
            .state
            .compare_exchange(UNLOCKED, ONE_WRITER, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            self.lock_exclusive_slow();
        }
    }

    /// Releases the exclusive lock. Wakes waiting threads if necessary.
    pub fn unlock_exclusive(&self) {
        if self
            .inner()
            .state
            .compare_exchange(ONE_WRITER, UNLOCKED, Ordering::Release, Ordering::Relaxed)
            .is_err()
        {
            self.unlock_exclusive_slow();
        }
    }

    /// Slow path: handles contention during exclusive lock acquisition
    #[cold]
    fn lock_exclusive_slow(&self) {
        let inner = self.inner();
        let mut acquire_with = UNLOCKED;
        let backoff = Backoff::new();
        let mut state = inner.state.load(Ordering::Relaxed);

        loop {
            while state & ONE_WRITER == 0 {
                match inner.state.compare_exchange_weak(
                    state,
                    state | ONE_WRITER | acquire_with,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return,
                    Err(e) => state = e,
                }
            }

            // Mark that writers are parked (only once)
            if state & WRITERS_PARKED == 0 {
                if !backoff.is_completed() {
                    backoff.snooze();
                    continue;
                }

                if let Err(e) = inner.state.compare_exchange_weak(
                    state,
                    state | WRITERS_PARKED,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    state = e;
                    continue;
                }
            }

            // Wait for a futex signal (i.e., writer release)
            loop {
                let w_key = inner.writers_futex.load(Ordering::Acquire);

                let state = inner.state.load(Ordering::Acquire);
                if (state & ONE_WRITER == 0) || (state & WRITERS_PARKED == 0) {
                    break;
                }

                futex_wait(&inner.writers_futex, w_key);
            }

            backoff.reset();
            acquire_with = WRITERS_PARKED;
            state = inner.state.load(Ordering::Relaxed);
        }
    }

    /// Slow path: handles release of exclusive lock when threads are waiting
    #[inline]
    fn unlock_exclusive_slow(&self) {
        let inner = self.inner();
        let parked = inner.state.swap(UNLOCKED, Ordering::Release);

        if parked & WRITERS_PARKED != 0 {
            // Prioritize writers
            inner.writers_futex.fetch_add(1, Ordering::Release);
            futex_wake(&*inner.writers_futex);
        }

        if parked & READERS_PARKED != 0 {
            // Wake all waiting readers
            inner.readers_futex.fetch_add(1, Ordering::Release);
            futex_wake_all(&*inner.readers_futex);
        }
    }

    /// Attempts to acquire a shared (reader) lock without blocking
    #[inline]
    pub fn try_lock_shared(&self) -> bool {
        self.try_lock_shared_fast() || self.try_lock_shared_slow()
    }

    /// Fast path: attempts to increment the reader count
    #[inline(always)]
    fn try_lock_shared_fast(&self) -> bool {
        let inner = self.inner();
        let state = inner.state.load(Ordering::Relaxed);

        if let Some(new_state) = state.checked_add(ONE_READER)
            && new_state & ONE_WRITER != ONE_WRITER
        {
            return inner
                .state
                .compare_exchange(state, new_state, Ordering::Acquire, Ordering::Relaxed)
                .is_ok();
        }

        false
    }

    /// Slow path: tries harder to acquire a shared lock under contention
    #[cold]
    fn try_lock_shared_slow(&self) -> bool {
        let inner = self.inner();
        let mut state = inner.state.load(Ordering::Relaxed);

        while let Some(new_state) = state.checked_add(ONE_READER) {
            if new_state & ONE_WRITER == ONE_WRITER {
                break;
            }

            match inner.state.compare_exchange_weak(
                state,
                new_state,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(e) => state = e,
            }
        }

        false
    }

    /// Acquires a shared (reader) lock, blocking if necessary
    pub fn lock_shared(&self) {
        if !self.try_lock_shared_fast() {
            self.lock_shared_slow();
        }
    }

    /// Releases a shared (reader) lock
    pub fn unlock_shared(&self) {
        let inner = self.inner();
        let prev_state = inner.state.fetch_sub(ONE_READER, Ordering::Release);

        // If this was the last reader and writers are waiting, wake one writer
        if prev_state == (ONE_READER | WRITERS_PARKED)
            && inner
            .state
            .compare_exchange(
                WRITERS_PARKED,
                UNLOCKED,
                Ordering::Release,
                Ordering::Relaxed,
            )
            .is_ok()
        {
            inner.writers_futex.fetch_add(1, Ordering::Release);
            futex_wake(&*inner.writers_futex);
        }
    }

    /// Acquires a shared lock with full contention handling (slow path)
    #[cold]
    fn lock_shared_slow(&self) {
        let inner = self.inner();
        let backoff = Backoff::new();

        loop {
            let mut state = inner.state.load(Ordering::Relaxed);

            while let Some(new_state) = state.checked_add(ONE_READER) {
                if inner
                    .state
                    .compare_exchange_weak(state, new_state, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return;
                }

                state = inner.state.load(Ordering::Relaxed);
            }

            // Mark that readers are parked
            if state & READERS_PARKED == 0 {
                if !backoff.is_completed() {
                    backoff.snooze();
                    continue;
                }

                if inner
                    .state
                    .compare_exchange_weak(
                        state,
                        state | READERS_PARKED,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    )
                    .is_err()
                {
                    continue;
                }
            }

            // Wait for a writer to release
            let w_key = inner.readers_futex.load(Ordering::Acquire);

            // Check condition BEFORE waiting
            let state = inner.state.load(Ordering::Acquire);
            if state & ONE_WRITER != ONE_WRITER {
                // Lock available, retry acquisition
                backoff.reset();
                continue;
            }

            futex_wait(&inner.readers_futex, w_key);

            // After waking, always retry from the top
            backoff.reset();
        }
    }

    /// Releases all held shared locks at once (bulk reader release)
    pub fn unlock_all_shared(&self) {
        let inner = self.inner();

        loop {
            let state = inner.state.load(Ordering::Acquire);
            let count = readers_count(state);
            if count == 0 {
                return;
            }

            let readers_to_remove = ONE_READER.wrapping_mul(count);
            let new_state = state.wrapping_sub(readers_to_remove);

            debug_assert!(
                !is_writer_locked(new_state),
                "unlock_all_shared resulted in writer lock set"
            );
            debug_assert!(
                readers_count(new_state) == 0,
                "unlock_all_shared didn't remove all readers"
            );

            if inner
                .state
                .compare_exchange(state, new_state, Ordering::Release, Ordering::Relaxed)
                .is_ok()
            {
                if readers_count(new_state) == 0 && (state & WRITERS_PARKED != 0) {
                    inner.writers_futex.fetch_add(1, Ordering::Release);
                    futex_wake(&*inner.writers_futex);
                }
                break;
            }
        }
    }

    /// Downgrades a writer lock to a reader lock without releasing ownership
    pub fn downgrade(&self) {
        let inner = self.inner();
        let mut state = inner.state.load(Ordering::Relaxed);

        loop {
            let new_state = (state & !ONE_WRITER) + ONE_READER;
            match inner.state.compare_exchange(
                state,
                new_state,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    if state & READERS_PARKED != 0 {
                        futex_wake_all(&*inner.readers_futex);
                    }
                    break;
                }
                Err(s) => state = s,
            }
        }
    }
}

// Clone increases the internal reference count
impl Clone for Mutex {
    fn clone(&self) -> Self {
        self.inner().ref_count.fetch_add(1, Ordering::Relaxed);
        Mutex { ptr: self.ptr }
    }
}

// Drop decreases the reference count and deallocates if this is the last owner
impl Drop for Mutex {
    fn drop(&mut self) {
        if self.inner().ref_count.fetch_sub(1, Ordering::Release) == 1 {
            atomic::fence(Ordering::Acquire);
            let ptr = self.ptr as *mut InnerMutex;
            unsafe { drop(Box::from_raw(ptr)) };
        }
    }
}

// Support for Mutex::default()
impl Default for Mutex {
    fn default() -> Self {
        Self::new()
    }
}

// Implements Debug formatting for inspecting internal state
impl fmt::Debug for Mutex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.inner().state.load(Ordering::Acquire);
        let readers = readers_count(state);

        f.debug_struct("Mutex")
            .field("state", &format!("{:b}", state))
            .field("exclusive_locked", &self.is_locked_exclusive())
            .field("readers_count", &readers)
            .field("readers_parked", &(state & READERS_PARKED != 0))
            .field("writers_parked", &(state & WRITERS_PARKED != 0))
            .field("ref_count", &self.inner().ref_count.load(Ordering::Acquire))
            .finish()
    }
}
