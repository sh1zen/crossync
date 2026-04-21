use crate::sync::{Backoff, RawMutex, WatchGuardMut, WatchGuardRef};
use crossbeam_utils::CachePadded;
use std::cell::UnsafeCell;
use std::fmt;
use std::iter::FromIterator;
use std::mem::{self, MaybeUninit};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering, fence};

/// Default capacity for the array
const DEFAULT_ARRAY_CAP: usize = 32;

/// Slot state flags (u32 for memory efficiency and cache usage)
const EMPTY: usize = 0;
const WRITE: usize = 1;
const READ: usize = 2;

/// Represents a single slot in the array.
struct Slot<T> {
    value: UnsafeCell<MaybeUninit<T>>,
    state: AtomicUsize,
    lock: RawMutex,
}

impl<T> Slot<T> {
    #[inline(always)]
    fn new() -> Self {
        Self {
            value: UnsafeCell::new(MaybeUninit::uninit()),
            state: AtomicUsize::new(EMPTY),
            lock: RawMutex::new(),
        }
    }

    /// Wait until this slot is written, with fast-path check and backoff spin
    #[inline(always)]
    fn wait_write(&self) {
        if self.state.load(Ordering::Acquire) & WRITE != 0 {
            return;
        }
        let backoff = Backoff::new();
        while self.state.load(Ordering::Acquire) & WRITE == 0 {
            backoff.snooze();
        }
    }

    /// Reset slot to EMPTY state
    #[inline(always)]
    fn reset(&self) {
        self.state.store(EMPTY, Ordering::Relaxed);
    }

    /// Return true if slot contains written data
    #[inline(always)]
    fn is_written(&self) -> bool {
        self.state.load(Ordering::Relaxed) & WRITE != 0
    }
}

/// Internal structure representing the array
struct ArrayStorage<T> {
    slots: *mut Slot<T>,
    capacity: usize,
}

struct InnerArray<T> {
    storage: UnsafeCell<ArrayStorage<T>>,
    head: CachePadded<AtomicUsize>,
    tail: CachePadded<AtomicUsize>,
    len: CachePadded<AtomicUsize>,
    coord_lock: RawMutex,
    ref_count: CachePadded<AtomicUsize>,
}

struct ArrayCoordGuard<'a> {
    lock: &'a RawMutex,
    exclusive: bool,
}

impl<'a> ArrayCoordGuard<'a> {
    fn shared(lock: &'a RawMutex) -> Self {
        lock.lock_shared();
        Self {
            lock,
            exclusive: false,
        }
    }

    fn exclusive(lock: &'a RawMutex) -> Self {
        lock.lock_exclusive();
        Self {
            lock,
            exclusive: true,
        }
    }
}

impl Drop for ArrayCoordGuard<'_> {
    fn drop(&mut self) {
        if self.exclusive {
            self.lock.unlock_exclusive();
        } else {
            self.lock.unlock_shared();
        }
    }
}

/// Thread-safe atomic array wrapper
#[repr(transparent)]
pub struct AtomicArray<T> {
    inner: *const InnerArray<T>,
}

unsafe impl<T: Send> Send for AtomicArray<T> {}
unsafe impl<T: Send> Sync for AtomicArray<T> {}

impl<T> AtomicArray<T> {
    /// Create new array with default capacity
    #[inline]
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_ARRAY_CAP)
    }

    /// Create new array with specific capacity
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(capacity > 0, "Capacity must be greater than 0");

        let inner = InnerArray {
            storage: UnsafeCell::new(ArrayStorage {
                slots: Self::allocate_slots(capacity),
                capacity,
            }),
            head: CachePadded::new(AtomicUsize::new(0)),
            tail: CachePadded::new(AtomicUsize::new(0)),
            len: CachePadded::new(AtomicUsize::new(0)),
            coord_lock: RawMutex::new(),
            ref_count: CachePadded::new(AtomicUsize::new(1)),
        };

        Self {
            inner: Box::into_raw(Box::new(inner)),
        }
    }

    /// Initialize array with given capacity and initializer function
    pub fn init_with<F: FnMut() -> T>(cap: usize, mut initializer: F) -> Self {
        let arr = Self::with_capacity(cap);
        for _ in 0..cap {
            let _ = arr.push(initializer());
        }
        arr
    }

    #[inline(always)]
    fn inner(&self) -> &InnerArray<T> {
        unsafe { &*self.inner }
    }

    #[inline(always)]
    unsafe fn storage(inner: &InnerArray<T>) -> &ArrayStorage<T> {
        unsafe { &*inner.storage.get() }
    }

    fn allocate_slots(capacity: usize) -> *mut Slot<T> {
        let layout = std::alloc::Layout::from_size_align(
            capacity * mem::size_of::<Slot<T>>(),
            mem::align_of::<Slot<T>>(),
        )
        .expect("Failed to create layout");

        unsafe {
            let ptr = std::alloc::alloc_zeroed(layout) as *mut Slot<T>;
            if ptr.is_null() {
                std::alloc::handle_alloc_error(layout);
            }
            for i in 0..capacity {
                ptr.add(i).write(Slot::new());
            }
            ptr
        }
    }

    unsafe fn deallocate_slots(slots: *mut Slot<T>, capacity: usize) {
        let layout = std::alloc::Layout::from_size_align(
            capacity * mem::size_of::<Slot<T>>(),
            mem::align_of::<Slot<T>>(),
        )
        .expect("Failed to create layout");
        unsafe {
            std::alloc::dealloc(slots as *mut u8, layout);
        }
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.inner().len.load(Ordering::Relaxed)
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline(always)]
    pub fn capacity(&self) -> usize {
        let inner = self.inner();
        let _coord = ArrayCoordGuard::shared(&inner.coord_lock);
        unsafe { Self::storage(inner).capacity }
    }

    /// Push a new element (fast path)
    #[inline]
    pub fn push(&self, value: T) -> Result<(), T> {
        let inner = self.inner();
        let _coord = ArrayCoordGuard::shared(&inner.coord_lock);
        let storage = unsafe { Self::storage(inner) };
        let tail = inner.tail.load(Ordering::Relaxed);
        if tail >= storage.capacity {
            return Err(value);
        }

        if inner
            .tail
            .compare_exchange(tail, tail + 1, Ordering::Release, Ordering::Acquire)
            .is_ok()
        {
            unsafe {
                let slot = &*storage.slots.add(tail);
                ptr::write(slot.value.get(), MaybeUninit::new(value));
                slot.state.store(WRITE, Ordering::Release);
                inner.len.fetch_add(1, Ordering::Relaxed);
            }
            return Ok(());
        }

        self.push_slow(inner, value)
    }

    /// Slow path push using backoff
    #[cold]
    fn push_slow(&self, inner: &InnerArray<T>, value: T) -> Result<(), T> {
        let backoff = Backoff::new();
        let mut tail = inner.tail.load(Ordering::Relaxed);
        let storage = unsafe { Self::storage(inner) };

        loop {
            if tail >= storage.capacity {
                return Err(value);
            }
            match inner
                .tail
                .compare_exchange(tail, tail + 1, Ordering::Release, Ordering::Acquire)
            {
                Ok(_) => {
                    unsafe {
                        let slot = &*storage.slots.add(tail);
                        ptr::write(slot.value.get(), MaybeUninit::new(value));
                        slot.state.store(WRITE, Ordering::Release);
                        inner.len.fetch_add(1, Ordering::Relaxed);
                    }
                    return Ok(());
                }
                Err(t) => {
                    tail = t;
                    backoff.snooze();
                }
            }
        }
    }

    /// Access element by index (shared)
    #[inline]
    pub fn get(&self, index: usize) -> Option<WatchGuardRef<'_, T>> {
        let inner = self.inner();
        let coord = ArrayCoordGuard::shared(&inner.coord_lock);
        let storage = unsafe { Self::storage(inner) };
        let len = inner.len.load(Ordering::Relaxed);
        if index >= len {
            return None;
        }

        let target = inner.head.load(Ordering::Relaxed) + index;
        if target >= storage.capacity {
            return None;
        }

        unsafe {
            let slot = &*storage.slots.add(target);
            slot.wait_write();
            slot.lock.lock_shared();
            let guard = WatchGuardRef::new((*slot.value.get()).assume_init_ref(), &slot.lock);
            drop(coord);
            Some(guard)
        }
    }

    /// Executes a closure while holding a shared guard on the element at `index`.
    #[inline]
    pub fn with<R>(&self, index: usize, f: impl FnOnce(&T) -> R) -> Option<R> {
        self.get(index).map(|guard| f(&guard))
    }

    /// Access element by index (mutable/exclusive)
    #[inline]
    pub fn get_mut(&self, index: usize) -> Option<WatchGuardMut<'_, T>> {
        let inner = self.inner();
        let coord = ArrayCoordGuard::shared(&inner.coord_lock);
        let storage = unsafe { Self::storage(inner) };
        let len = inner.len.load(Ordering::Relaxed);
        if index >= len {
            return None;
        }

        let target = inner.head.load(Ordering::Relaxed) + index;
        if target >= storage.capacity {
            return None;
        }

        unsafe {
            let slot = &*storage.slots.add(target);
            slot.wait_write();
            slot.lock.lock_exclusive();
            let guard = WatchGuardMut::new((*slot.value.get()).assume_init_mut(), &slot.lock);
            drop(coord);
            Some(guard)
        }
    }

    /// Executes a closure while holding an exclusive guard on the element at `index`.
    #[inline]
    pub fn with_mut<R>(&self, index: usize, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        self.get_mut(index).map(|mut guard| f(&mut guard))
    }

    /// Reset array with new capacity and initializer
    pub fn reset_with(
        &self,
        new_cap: usize,
        mut initializer: impl FnMut() -> T,
    ) -> Result<usize, usize> {
        assert!(new_cap > 0, "Capacity must be greater than 0");

        let inner_ptr = self.inner as *mut InnerArray<T>;
        let _coord = unsafe { ArrayCoordGuard::exclusive(&(*inner_ptr).coord_lock) };
        let storage = unsafe { &mut *(*inner_ptr).storage.get() };
        let old_capacity = storage.capacity;
        let old_len = unsafe { (*inner_ptr).len.load(Ordering::Relaxed) };
        let old_head = unsafe { (*inner_ptr).head.load(Ordering::Relaxed) };
        let old_slots = storage.slots;

        unsafe {
            for i in 0..old_len {
                let idx = old_head + i;
                if idx >= old_capacity {
                    continue;
                }

                let slot = &*old_slots.add(idx);
                if !slot.is_written() {
                    continue;
                }

                slot.lock.lock_exclusive();
                if mem::needs_drop::<T>() {
                    ptr::drop_in_place((*slot.value.get()).as_mut_ptr());
                }
                slot.reset();
                slot.lock.unlock_exclusive();
            }

            for i in 0..old_capacity {
                ptr::drop_in_place(old_slots.add(i));
            }
        }
        unsafe { Self::deallocate_slots(old_slots, old_capacity) };

        unsafe {
            let slots = Self::allocate_slots(new_cap);
            storage.slots = slots;
            storage.capacity = new_cap;
            (*inner_ptr).head.store(0, Ordering::Relaxed);
            (*inner_ptr).tail.store(0, Ordering::Relaxed);
            (*inner_ptr).len.store(0, Ordering::Relaxed);

            for i in 0..new_cap {
                let value = initializer();
                let slot = &*slots.add(i);
                ptr::write(slot.value.get(), MaybeUninit::new(value));
                slot.state.store(WRITE, Ordering::Release);
            }

            (*inner_ptr).tail.store(new_cap, Ordering::Relaxed);
            (*inner_ptr).len.store(new_cap, Ordering::Relaxed);
        }

        Ok(new_cap)
    }

    /// Convert to Vec<T> (requires Clone)
    pub fn as_vec(&self) -> Vec<T>
    where
        T: Clone,
    {
        let inner = self.inner();
        let _coord = ArrayCoordGuard::shared(&inner.coord_lock);
        let storage = unsafe { Self::storage(inner) };
        let len = inner.len.load(Ordering::Relaxed);
        if len == 0 {
            return Vec::new();
        }

        let mut out = Vec::with_capacity(len);
        let head = inner.head.load(Ordering::Relaxed);

        unsafe {
            for i in 0..len {
                let target = head + i;
                if target < storage.capacity {
                    let slot = &*storage.slots.add(target);
                    slot.wait_write();
                    slot.lock.lock_shared();
                    out.push((*slot.value.get()).assume_init_ref().clone());
                    slot.lock.unlock_shared();
                }
            }
        }

        out
    }

    /// Apply shared function to each element
    #[inline]
    pub fn for_each<F>(&self, mut f: F)
    where
        F: FnMut(&T),
    {
        let inner = self.inner();
        let _coord = ArrayCoordGuard::shared(&inner.coord_lock);
        let storage = unsafe { Self::storage(inner) };
        let len = inner.len.load(Ordering::Relaxed);
        let head = inner.head.load(Ordering::Relaxed);

        unsafe {
            for i in 0..len {
                let slot = &*storage.slots.add(head + i);
                slot.wait_write();
                slot.lock.lock_shared();
                f((*slot.value.get()).assume_init_ref());
                slot.lock.unlock_shared();
            }
        }
    }

    /// Apply exclusive mutable function to each element
    #[inline]
    pub fn for_each_mut<F>(&self, mut f: F)
    where
        F: FnMut(&mut T),
    {
        let inner = self.inner();
        let _coord = ArrayCoordGuard::shared(&inner.coord_lock);
        let storage = unsafe { Self::storage(inner) };
        let len = inner.len.load(Ordering::Relaxed);
        let head = inner.head.load(Ordering::Relaxed);

        unsafe {
            for i in 0..len {
                let slot = &*storage.slots.add(head + i);
                slot.wait_write();
                slot.lock.lock_exclusive();
                f((*slot.value.get()).assume_init_mut());
                slot.lock.unlock_exclusive();
            }
        }
    }

    /// Split indices into chunks for parallel processing
    #[inline]
    pub fn chunk_indices(&self, num_chunks: usize) -> Vec<(usize, usize)> {
        let len = self.len();
        if len == 0 || num_chunks == 0 {
            return vec![];
        }

        let chunk_size = (len + num_chunks - 1) / num_chunks;
        let mut chunks = Vec::with_capacity(num_chunks);
        for i in 0..num_chunks {
            let start = i * chunk_size;
            let end = ((i + 1) * chunk_size).min(len);
            if start < len {
                chunks.push((start, end));
            }
        }
        chunks
    }
}

impl<T> Clone for AtomicArray<T> {
    #[inline]
    fn clone(&self) -> Self {
        self.inner().ref_count.fetch_add(1, Ordering::Relaxed);
        Self { inner: self.inner }
    }
}

impl<T> Drop for AtomicArray<T> {
    fn drop(&mut self) {
        let inner = unsafe { &*self.inner };
        if inner.ref_count.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        fence(Ordering::Acquire);

        unsafe {
            let storage = &*inner.storage.get();
            // Drop dei valori contenuti
            if mem::needs_drop::<T>() {
                let len = inner.len.load(Ordering::Acquire);
                let head = inner.head.load(Ordering::Acquire);
                for i in 0..len {
                    let idx = head + i;
                    if idx < storage.capacity {
                        let slot = &*storage.slots.add(idx);
                        if slot.is_written() {
                            // FIX: drop il valore T, non MaybeUninit<T>
                            ptr::drop_in_place((*slot.value.get()).as_mut_ptr());
                        }
                    }
                }
            }

            // Drop degli slot (RawMutex, AtomicUsize, etc.)
            for i in 0..storage.capacity {
                ptr::drop_in_place(storage.slots.add(i));
            }

            // Dealloca memoria slot
            let layout = std::alloc::Layout::from_size_align(
                storage.capacity * mem::size_of::<Slot<T>>(),
                mem::align_of::<Slot<T>>(),
            )
            .expect("Failed to create layout");
            std::alloc::dealloc(storage.slots as *mut u8, layout);

            // Drop InnerArray
            drop(Box::from_raw(self.inner as *mut InnerArray<T>));
        }
    }
}

impl<T> FromIterator<T> for AtomicArray<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let capacity = iter.size_hint().0.max(DEFAULT_ARRAY_CAP);
        let arr = Self::with_capacity(capacity);
        for item in iter {
            let _ = arr.push(item);
        }
        arr
    }
}

impl<T> Default for AtomicArray<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: fmt::Debug> fmt::Debug for AtomicArray<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AtomicArray")
            .field("len", &self.len())
            .field("capacity", &self.capacity())
            .finish()
    }
}
