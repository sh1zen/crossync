use crate::sync::Backoff;
use crossbeam_utils::CachePadded;
use std::cell::UnsafeCell;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering, fence};

struct Slot<T> {
    sequence: AtomicUsize,
    value: UnsafeCell<*mut T>,
}
impl<T> Drop for Slot<T> {
    fn drop(&mut self) {
        let value = *self.value.get_mut();
        if !value.is_null() {
            unsafe {
                drop(Box::from_raw(value));
            }
        }
    }
}
struct AtomicBufferInner<T> {
    head: CachePadded<AtomicUsize>,
    tail: CachePadded<AtomicUsize>,
    slots: Box<[Slot<T>]>,
    single: AtomicPtr<T>,
    ref_count: AtomicUsize,
    cap: usize,
}
impl<T> Drop for AtomicBufferInner<T> {
    fn drop(&mut self) {
        let value = *self.single.get_mut();
        if !value.is_null() {
            unsafe {
                drop(Box::from_raw(value));
            }
        }
    }
}

/// Bounded MPMC queue owning transferred allocations. Per-slot sequence
/// numbers distinguish successive ring generations. A stalled reservation
/// may delay other operations; this queue does not promise lock-free progress.
#[repr(transparent)]
pub struct AtomicBuffer<T> {
    inner: *const AtomicBufferInner<T>,
}
unsafe impl<T: Send> Send for AtomicBuffer<T> {}
unsafe impl<T: Send> Sync for AtomicBuffer<T> {}

impl<T> AtomicBuffer<T> {
    pub fn new() -> Self {
        Self::with_capacity(32)
    }
    pub fn with_capacity(cap: usize) -> Self {
        assert!(cap.is_power_of_two(), "capacity must be power of two");
        assert!(cap <= isize::MAX as usize, "capacity is too large");
        let slots = (0..cap)
            .map(|i| Slot {
                sequence: AtomicUsize::new(i),
                value: UnsafeCell::new(ptr::null_mut()),
            })
            .collect();
        Self {
            inner: Box::into_raw(Box::new(AtomicBufferInner {
                head: CachePadded::new(AtomicUsize::new(0)),
                tail: CachePadded::new(AtomicUsize::new(0)),
                slots,
                single: AtomicPtr::new(ptr::null_mut()),
                ref_count: AtomicUsize::new(1),
                cap,
            })),
        }
    }
    #[inline(always)]
    fn inner(&self) -> &AtomicBufferInner<T> {
        unsafe { &*self.inner }
    }

    /// Transfers ownership on success. Null is rejected without reserving space.
    ///
    /// # Safety
    /// A non-null pointer must come from Box::into_raw or an equivalent allocation
    /// and have unique ownership. After success it must not be accessed or freed
    /// until returned by pop/drain. On failure the caller retains ownership.
    ///
    /// ```compile_fail
    /// let buffer = crossync::atomic::AtomicBuffer::new();
    /// buffer.push(Box::into_raw(Box::new(1)));
    /// ```
    #[inline]
    pub unsafe fn push(&self, value: *mut T) -> Result<(), *mut T> {
        if value.is_null() {
            return Err(value);
        }
        let inner = self.inner();
        if inner.cap == 1 {
            return inner
                .single
                .compare_exchange(ptr::null_mut(), value, Ordering::Release, Ordering::Relaxed)
                .map(|_| ())
                .map_err(|_| value);
        }
        let backoff = Backoff::new();
        let mut pos = inner.tail.load(Ordering::Relaxed);
        loop {
            let slot = &inner.slots[pos & (inner.cap - 1)];
            let seq = slot.sequence.load(Ordering::Acquire);
            let diff = seq.wrapping_sub(pos) as isize;
            if diff == 0 {
                match inner.tail.compare_exchange_weak(
                    pos,
                    pos.wrapping_add(1),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        unsafe {
                            *slot.value.get() = value;
                        }
                        slot.sequence.store(pos.wrapping_add(1), Ordering::Release);
                        return Ok(());
                    }
                    Err(current) => pos = current,
                }
            } else if diff < 0 {
                return Err(value);
            } else {
                pos = inner.tail.load(Ordering::Relaxed);
            }
            backoff.spin();
        }
    }
    /// Safe ownership transfer without raw pointers.
    #[inline]
    pub fn push_box(&self, value: Box<T>) -> Result<(), Box<T>> {
        unsafe {
            self.push(Box::into_raw(value))
                .map_err(|p| Box::from_raw(p))
        }
    }
    /// Returns unique ownership of the oldest allocation.
    #[inline]
    pub fn pop(&self) -> Option<*mut T> {
        let inner = self.inner();
        if inner.cap == 1 {
            let value = inner.single.swap(ptr::null_mut(), Ordering::Acquire);
            return (!value.is_null()).then_some(value);
        }
        let backoff = Backoff::new();
        let mut pos = inner.head.load(Ordering::Relaxed);
        loop {
            let slot = &inner.slots[pos & (inner.cap - 1)];
            let seq = slot.sequence.load(Ordering::Acquire);
            let diff = seq.wrapping_sub(pos.wrapping_add(1)) as isize;
            if diff == 0 {
                match inner.head.compare_exchange_weak(
                    pos,
                    pos.wrapping_add(1),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        let value = unsafe { ptr::replace(slot.value.get(), ptr::null_mut()) };
                        slot.sequence
                            .store(pos.wrapping_add(inner.cap), Ordering::Release);
                        return Some(value);
                    }
                    Err(current) => pos = current,
                }
            } else if diff < 0 {
                if inner.tail.load(Ordering::Acquire) == pos {
                    return None;
                }
                pos = inner.head.load(Ordering::Relaxed);
                backoff.snooze();
            } else {
                pos = inner.head.load(Ordering::Relaxed);
            }
        }
    }
    #[inline]
    pub fn pop_box(&self) -> Option<Box<T>> {
        self.pop().map(|p| unsafe { Box::from_raw(p) })
    }
    #[inline]
    pub fn try_pop_weak(&self) -> Option<*mut T> {
        self.pop()
    }
    #[inline]
    pub fn capacity(&self) -> usize {
        self.inner().cap
    }
    #[inline]
    pub fn is_empty_fast(&self) -> bool {
        self.len_approx() == 0
    }
    /// Approximate size under contention, always bounded by capacity.
    #[inline]
    pub fn len_approx(&self) -> usize {
        let inner = self.inner();
        if inner.cap == 1 {
            return usize::from(!inner.single.load(Ordering::Relaxed).is_null());
        }
        let head = inner.head.load(Ordering::Relaxed);
        inner
            .tail
            .load(Ordering::Relaxed)
            .wrapping_sub(head)
            .min(inner.cap)
    }
    /// Lazily removes up to capacity entries; dropping the iterator preserves
    /// unconsumed entries and their ownership in the queue.
    pub fn drain_all(&self) -> impl Iterator<Item = *mut T> + '_ {
        std::iter::from_fn(move || self.pop()).take(self.capacity())
    }
    pub fn drain_to_vec(&self) -> Vec<*mut T> {
        self.drain_all().collect()
    }
}
impl<T> Default for AtomicBuffer<T> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T> Clone for AtomicBuffer<T> {
    fn clone(&self) -> Self {
        crate::core::increment_ref_count(&self.inner().ref_count);
        Self { inner: self.inner }
    }
}
impl<T> Drop for AtomicBuffer<T> {
    fn drop(&mut self) {
        if self.inner().ref_count.fetch_sub(1, Ordering::Release) == 1 {
            fence(Ordering::Acquire);
            unsafe {
                drop(Box::from_raw(self.inner.cast_mut()));
            }
        }
    }
}
