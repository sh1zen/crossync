use crate::core::serial::{SerialGuard, SerialMutex};
use crate::sync::Backoff;
use crossbeam_utils::CachePadded;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::{cell::Cell, marker::PhantomData, rc::Rc};

const LANES: usize = 64;

/// Shared operation pinning without a globally contended reader count.
/// A writer publishes its intent before checking every lane. The sequentially
/// consistent increment/recheck and intent/scan pair ensure either the writer
/// sees the pin or the reader sees intent and retries before touching storage.
pub(crate) struct Coord {
    readers: [CachePadded<AtomicUsize>; LANES + 1],
    owners: [AtomicUsize; LANES],
    id: usize,
    writing: CachePadded<AtomicBool>,
    writers: SerialMutex,
}
pub(crate) struct Shared<'a> {
    count: &'a AtomicUsize,
    owned: bool,
    _thread: PhantomData<Rc<()>>,
}
pub(crate) struct Exclusive<'a> {
    coord: &'a Coord,
    _writer: SerialGuard<'a>,
}

impl Coord {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        Self {
            readers: std::array::from_fn(|_| CachePadded::new(AtomicUsize::new(0))),
            owners: std::array::from_fn(|_| AtomicUsize::new(0)),
            id: NEXT
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                .expect("coord identifier overflow"),
            writing: CachePadded::new(AtomicBool::new(false)),
            writers: SerialMutex::new(),
        }
    }
    #[inline(always)]
    pub(crate) fn shared(&self) -> Shared<'_> {
        thread_local! {
            static LAST: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
        }
        let lane = LAST.with(|last| {
            let (id, lane) = last.get();
            if id == self.id {
                lane
            } else {
                let lane = self.register();
                last.set((self.id, lane));
                lane
            }
        });
        let count = &self.readers[lane];
        let owned = lane < LANES;
        let backoff = Backoff::new();
        loop {
            count.fetch_add(1, Ordering::SeqCst);
            if !self.writing.load(Ordering::SeqCst) {
                return Shared {
                    count,
                    owned,
                    _thread: PhantomData,
                };
            }
            if owned {
                count.store(count.load(Ordering::Relaxed) - 1, Ordering::Release);
            } else {
                count.fetch_sub(1, Ordering::Release);
            }
            assert!(
                !self.writers.owned_by_current_thread(),
                "reentrant AtomicVec access"
            );
            while self.writing.load(Ordering::Acquire) {
                backoff.snooze();
            }
            backoff.snooze();
        }
    }
    #[cold]
    fn register(&self) -> usize {
        thread_local! {
            static ID: usize = {
                static NEXT: AtomicUsize = AtomicUsize::new(1);
                NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1)).expect("thread identifier overflow")
            };
        }
        let id = ID.with(|id| *id);
        for offset in 0..LANES {
            let lane = id.wrapping_add(offset) & (LANES - 1);
            match self.owners[lane].compare_exchange(0, id, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => return lane,
                Err(owner) if owner == id => return lane,
                Err(_) => {}
            }
        }
        LANES // Oversubscription uses a shared atomic counter, never another thread's lane.
    }
    pub(crate) fn exclusive(&self) -> Exclusive<'_> {
        let writer = self.writers.write_guard();
        self.writing.store(true, Ordering::SeqCst);
        let backoff = Backoff::new();
        for count in &self.readers {
            while count.load(Ordering::SeqCst) != 0 {
                backoff.snooze();
            }
        }
        Exclusive {
            coord: self,
            _writer: writer,
        }
    }
    pub(crate) fn try_exclusive(&self) -> Option<Exclusive<'_>> {
        let writer = self.writers.try_write_guard()?;
        self.writing.store(true, Ordering::SeqCst);
        if self
            .readers
            .iter()
            .any(|count| count.load(Ordering::SeqCst) != 0)
        {
            self.writing.store(false, Ordering::Release);
            return None;
        }
        Some(Exclusive {
            coord: self,
            _writer: writer,
        })
    }
}
impl Drop for Shared<'_> {
    #[inline]
    fn drop(&mut self) {
        if self.owned {
            self.count
                .store(self.count.load(Ordering::Relaxed) - 1, Ordering::Release);
        } else {
            self.count.fetch_sub(1, Ordering::Release);
        }
    }
}
impl Drop for Exclusive<'_> {
    fn drop(&mut self) {
        self.coord.writing.store(false, Ordering::Release);
    }
}
