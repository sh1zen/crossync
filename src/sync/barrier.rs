use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

struct State {
    remaining: usize,
    generation: usize,
}
struct Inner {
    state: Mutex<State>,
    cond: Condvar,
    count: AtomicUsize,
    bucket: usize,
}

/// Cloneable barrier. Each generation releases exactly its required arrivals.
/// Sharing through Arc or borrowed references has the same behavior as cloning.
#[derive(Clone)]
pub struct Barrier {
    inner: Arc<Inner>,
}

impl Barrier {
    pub fn new() -> Self {
        Self::with_capacity(1, 0)
    }

    /// The first phase requires n arrivals; following phases require bucket.
    /// A zero bucket disables the barrier after the first phase. A zero n
    /// creates an already-disabled barrier.
    pub fn with_capacity(n: usize, bucket: usize) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    remaining: n,
                    generation: 0,
                }),
                cond: Condvar::new(),
                count: AtomicUsize::new(n),
                bucket,
            }),
        }
    }

    pub fn count(&self) -> usize {
        self.inner.count.load(Ordering::Acquire)
    }

    pub fn wait(&self) {
        let inner = &self.inner;
        let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.remaining == 0 {
            return;
        }
        let generation = state.generation;
        state.remaining -= 1;
        inner.count.store(state.remaining, Ordering::Release);
        if state.remaining == 0 {
            state.remaining = inner.bucket;
            state.generation = state.generation.wrapping_add(1);
            inner.count.store(state.remaining, Ordering::Release);
            inner.cond.notify_all();
        } else {
            while state.generation == generation {
                state = inner.cond.wait(state).unwrap_or_else(|e| e.into_inner());
            }
        }
    }

    /// Completes the current phase and releases its waiters, even if some of
    /// its required participants have not arrived yet.
    pub fn release(&self) {
        let inner = &self.inner;
        let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
        state.remaining = inner.bucket;
        state.generation = state.generation.wrapping_add(1);
        inner.count.store(state.remaining, Ordering::Release);
        inner.cond.notify_all();
    }
}
impl Default for Barrier {
    fn default() -> Self {
        Self::new()
    }
}
