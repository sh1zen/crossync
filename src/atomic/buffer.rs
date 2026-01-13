use crossbeam_utils::CachePadded;
use std::ptr;
use std::sync::atomic::{fence, AtomicPtr, AtomicUsize, Ordering};

struct AtomicBufferInner<T> {
    // Hot path: separate cache lines for producer and consumer
    head: CachePadded<AtomicUsize>,  // Consumer side
    tail: CachePadded<AtomicUsize>,  // Producer side
    slots: Box<[CachePadded<AtomicPtr<T>>]>,
    // Cold data
    ref_count: CachePadded<AtomicUsize>,
    cap: usize,
    cap_mask: usize,
}

unsafe impl<T: Send> Send for AtomicBuffer<T> {}
unsafe impl<T: Send> Sync for AtomicBuffer<T> {}

#[repr(transparent)]
pub struct AtomicBuffer<T> {
    inner: *const AtomicBufferInner<T>,
}

impl<T> AtomicBuffer<T> {
    pub fn new() -> Self {
        Self::with_capacity(32)
    }

    pub fn with_capacity(cap: usize) -> Self {
        assert!(cap.is_power_of_two(), "capacity must be power of two");

        // Pre-allocate with exact capacity, avoid reallocation
        let slots: Box<[_]> = (0..cap)
            .map(|_| CachePadded::new(AtomicPtr::new(ptr::null_mut())))
            .collect();

        let inner = AtomicBufferInner {
            head: CachePadded::new(AtomicUsize::new(0)),
            tail: CachePadded::new(AtomicUsize::new(0)),
            slots,
            ref_count: CachePadded::new(AtomicUsize::new(1)),
            cap,
            cap_mask: cap - 1,
        };

        Self {
            inner: Box::into_raw(Box::new(inner)),
        }
    }

    #[inline(always)]
    fn inner(&self) -> &AtomicBufferInner<T> {
        // SAFETY: inner is always valid while AtomicBuffer exists
        unsafe { &*self.inner }
    }

    /// Get slot without bounds checking (index is always masked)
    #[inline(always)]
    unsafe fn slot_unchecked(&self, idx: usize) -> &CachePadded<AtomicPtr<T>> {
        unsafe {
            self.inner().slots.get_unchecked(idx)
        }
    }

    #[inline]
    pub fn push(&self, ptr: *mut T) -> Result<(), *mut T> {
        let inner = self.inner();

        loop {
            let tail = inner.tail.load(Ordering::Relaxed);
            let head = inner.head.load(Ordering::Acquire);

            if tail.wrapping_sub(head) >= inner.cap {
                return Err(ptr);
            }

            // Riserva lo slot con CAS su tail
            if inner.tail.compare_exchange_weak(
                tail,
                tail.wrapping_add(1),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ).is_err() {
                continue; // Altro producer ha vinto, riprova
            }

            // Ora abbiamo riservato slot[tail]
            let idx = tail & inner.cap_mask;
            let slot = unsafe { self.slot_unchecked(idx) };

            // Scrivi il valore (lo slot potrebbe non essere ancora vuoto
            // se un consumer è lento)
            loop {
                match slot.compare_exchange_weak(
                    ptr::null_mut(),
                    ptr,
                    Ordering::Release,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return Ok(()),
                    Err(_) => core::hint::spin_loop(),
                }
            }
        }
    }

    #[inline]
    pub fn pop(&self) -> Option<*mut T> {
        let inner = self.inner();

        loop {
            let head = inner.head.load(Ordering::Relaxed);
            let tail = inner.tail.load(Ordering::Acquire);

            if head == tail {
                return None;
            }

            // Riserva lo slot con CAS su head
            if inner.head.compare_exchange_weak(
                head,
                head.wrapping_add(1),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ).is_err() {
                continue;
            }

            let idx = head & inner.cap_mask;
            let slot = unsafe { self.slot_unchecked(idx) };

            // Leggi il valore (potrebbe non essere ancora scritto)
            loop {
                let val = slot.swap(ptr::null_mut(), Ordering::Acquire);
                if !val.is_null() {
                    return Some(val);
                }
                core::hint::spin_loop();
            }
        }
    }

    /// Try pop without advancing head - useful for peek-like operations
    #[inline]
    pub fn try_pop_weak(&self) -> Option<*mut T> {
        let inner = self.inner();
        let head = inner.head.load(Ordering::Relaxed);
        let tail = inner.tail.load(Ordering::Relaxed);

        if head == tail {
            return None;
        }

        let idx = head & inner.cap_mask;
        let slot = unsafe { self.slot_unchecked(idx) };
        let val = slot.swap(ptr::null_mut(), Ordering::Acquire);

        if !val.is_null() {
            inner.head.store(head.wrapping_add(1), Ordering::Release);
            Some(val)
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.inner().cap
    }

    /// Fast check if buffer appears empty (may have false negatives under contention)
    #[inline(always)]
    pub fn is_empty_fast(&self) -> bool {
        let inner = self.inner();
        inner.head.load(Ordering::Relaxed) == inner.tail.load(Ordering::Relaxed)
    }

    /// Approximate length (may be slightly off under contention)
    #[inline]
    pub fn len_approx(&self) -> usize {
        let inner = self.inner();
        let tail = inner.tail.load(Ordering::Relaxed);
        let head = inner.head.load(Ordering::Relaxed);
        tail.wrapping_sub(head)
    }

    pub fn drain_all(&self) -> impl Iterator<Item = *mut T> + '_ {
        let inner = self.inner();

        // Reset completo degli indici
        inner.head.store(0, Ordering::Relaxed);
        inner.tail.store(0, Ordering::Release);

        inner.slots.iter().filter_map(|slot| {
            let ptr = slot.swap(ptr::null_mut(), Ordering::Acquire);
            if ptr.is_null() { None } else { Some(ptr) }
        })
    }

    /// Batch drain with pre-allocated vector - more efficient for large drains
    #[inline]
    pub fn drain_to_vec(&self) -> Vec<*mut T> {
        let inner = self.inner();
        let mut result = Vec::with_capacity(inner.cap);

        for slot in inner.slots.iter() {
            let ptr = slot.swap(ptr::null_mut(), Ordering::Acquire);
            if !ptr.is_null() {
                result.push(ptr);
            }
        }
        result
    }
}

impl<T> Default for AtomicBuffer<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Clone for AtomicBuffer<T> {
    #[inline]
    fn clone(&self) -> Self {
        // Relaxed is sufficient for increment - we don't need to synchronize data
        // The synchronization happens in Drop via the Acquire fence
        self.inner().ref_count.fetch_add(1, Ordering::Relaxed);
        Self { inner: self.inner }
    }
}

impl<T> Drop for AtomicBuffer<T> {
    fn drop(&mut self) {
        let inner = unsafe { &*self.inner };

        // Release: ensure all our writes are visible before potential deallocation
        if inner.ref_count.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }

        // Acquire fence: synchronize with all Release decrements from other threads
        fence(Ordering::Acquire);

        // Free all stored elements
        for slot in inner.slots.iter() {
            let ptr = slot.load(Ordering::Relaxed);
            if !ptr.is_null() {
                unsafe { drop(Box::from_raw(ptr)) };
            }
        }

        unsafe {
            drop(Box::from_raw(self.inner as *mut AtomicBufferInner<T>));
        }
    }
}