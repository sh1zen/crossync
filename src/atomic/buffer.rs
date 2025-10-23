use crossbeam_utils::CachePadded;
use std::ptr;
use std::sync::atomic::{fence, AtomicPtr, AtomicUsize, Ordering};

struct AtomicBufferInner<T> {
    head: CachePadded<AtomicUsize>,
    tail: CachePadded<AtomicUsize>,
    slots: Box<[CachePadded<AtomicPtr<T>>]>,
    ref_count: CachePadded<AtomicUsize>, // reference count for cloning
    cap: usize,
    cap_mask: usize,
}

unsafe impl<T: Send> Send for AtomicBuffer<T> {}
unsafe impl<T: Send> Sync for AtomicBuffer<T> {}

/// Thread-safe AtomicVec with atomic operations.
#[repr(transparent)]
pub struct AtomicBuffer<T> {
    inner: *const AtomicBufferInner<T>,
}

impl<T> AtomicBuffer<T> {
    /// Standard constructor with default capacity of 32
    pub fn new() -> Self {
        Self::with_capacity(32)
    }

    /// Constructor with specified capacity (must be a power of 2)
    pub fn with_capacity(cap: usize) -> Self {
        assert!(cap.is_power_of_two(), "capacity must be power of two");

        let mut slots = Vec::with_capacity(cap);
        for _ in 0..cap {
            // Initialize each slot as a null pointer, wrapped in CachePadded
            slots.push(CachePadded::new(AtomicPtr::new(ptr::null_mut())));
        }

        let inner = AtomicBufferInner {
            head: CachePadded::new(AtomicUsize::new(0)),
            tail: CachePadded::new(AtomicUsize::new(0)),
            slots: slots.into_boxed_slice(),
            ref_count: CachePadded::new(AtomicUsize::new(1)),
            cap,
            cap_mask: cap - 1, // used for fast modulo (index wrapping)
        };

        Self {
            inner: Box::into_raw(Box::new(inner)),
        }
    }

    #[inline(always)]
    fn inner(&self) -> &AtomicBufferInner<T> {
        unsafe { &*self.inner }
    }

    /// Attempt to push a pointer into the buffer
    #[inline]
    pub fn push(&self, ptr: *mut T) -> Result<(), *mut T> {
        let inner = self.inner();
        let tail = inner.tail.load(Ordering::Acquire);
        let head = inner.head.load(Ordering::Acquire);

        // Buffer full: cannot push more elements
        if tail.wrapping_sub(head) == inner.cap {
            return Err(ptr);
        }

        let idx = tail & inner.cap_mask; // calculate slot index using bitmask
        let slot = &inner.slots[idx];

        // Write only if slot is empty (CAS prevents overwrite)
        let prev =
            slot.compare_exchange(ptr::null_mut(), ptr, Ordering::Release, Ordering::Relaxed);

        if prev.is_ok() {
            inner.tail.store(tail.wrapping_add(1), Ordering::Release);
            Ok(())
        } else {
            // Someone else wrote into this slot concurrently
            Err(ptr)
        }
    }

    /// Attempt to pop a pointer from the buffer
    #[inline]
    pub fn pop(&self) -> Option<*mut T> {
        let inner = self.inner();
        let head = inner.head.load(Ordering::Acquire);
        let tail = inner.tail.load(Ordering::Acquire);

        // Buffer empty
        if head == tail {
            return None;
        }

        let idx = head & inner.cap_mask; // calculate slot index using bitmask
        let slot = &inner.slots[idx];

        // Swap the slot value with null; atomic operation ensures no races
        let val = slot.swap(ptr::null_mut(), Ordering::Acquire);

        if !val.is_null() {
            inner.head.store(head.wrapping_add(1), Ordering::Release);
            Some(val)
        } else {
            // Slot was unexpectedly empty (contention)
            None
        }
    }

    /// Returns the capacity of the buffer
    #[inline]
    pub fn capacity(&self) -> usize {
        self.inner().cap
    }
}

impl<T> Clone for AtomicBuffer<T> {
    /// Cloning the AtomicVec only increases the reference count.
    /// The underlying data is shared safely between clones.
    fn clone(&self) -> Self {
        let inner = self.inner();
        inner.ref_count.fetch_add(1, Ordering::Relaxed); // increment ref count
        Self { inner: self.inner }
    }
}

impl<T> Drop for AtomicBuffer<T> {
    /// Drops the AtomicVec.
    /// If this is the last reference, deallocates all blocks and inner structures.
    fn drop(&mut self) {
        let inner = unsafe { &*self.inner };

        // Only deallocate if this is the last reference
        if inner.ref_count.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        fence(Ordering::Acquire); // synchronize with other threads

        unsafe {
            // Deallocate AtomicBufferInner itself
            drop(Box::from_raw(self.inner as *mut AtomicBufferInner<T>));
        }
    }
}
