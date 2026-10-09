use crate::core::serial::SerialMutex;
use crate::sync::{WatchGuardMut, WatchGuardRef};
use crossbeam_utils::CachePadded;
use std::cell::UnsafeCell;
use std::mem::{self, MaybeUninit};
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::sync::atomic::{self, AtomicUsize, Ordering};

/// Internal representation of the atomic cell.
/// Provides interior mutability through a Mutex.
/// Reference counting allows cloning the AtomicCell.
struct Inner<T> {
    /// Value stored in the cell, possibly uninitialized
    val: UnsafeCell<MaybeUninit<T>>,
    /// Mutex protecting concurrent access
    state: SerialMutex,
    /// Reference count for clone/drop semantics
    ref_count: CachePadded<AtomicUsize>,
}

impl<T> Inner<T> {
    /// Creates a new Inner cell containing `val`
    fn new(val: T) -> Self {
        Self {
            val: UnsafeCell::new(MaybeUninit::new(val)),
            state: SerialMutex::new(),
            ref_count: CachePadded::new(AtomicUsize::new(1)),
        }
    }
}

// Read groups are confined to one acquiring thread; writes exclude all guards.
// Read guards themselves remain Send/Sync only for T: Sync.
unsafe impl<T: Send> Send for AtomicCell<T> {}
unsafe impl<T: Send> Sync for AtomicCell<T> {}

// Safe for unwind scenarios
impl<T> UnwindSafe for AtomicCell<T> {}
impl<T> RefUnwindSafe for AtomicCell<T> {}

/// Thread-safe atomic cell wrapper
#[repr(transparent)]
pub struct AtomicCell<T> {
    ptr: *const Inner<T>,
}

impl<T> AtomicCell<T> {
    /// Create a new atomic cell containing `val`
    pub fn new(val: T) -> Self {
        let inner = Box::new(Inner::new(val));
        let ptr = Box::into_raw(inner);
        Self { ptr }
    }

    /// Returns a reference to the inner struct
    #[inline(always)]
    fn inner(&self) -> &Inner<T> {
        unsafe { &*self.ptr }
    }

    /// Shared (read-only) access via sync
    #[inline]
    pub fn get(&self) -> WatchGuardRef<'_, T> {
        let lock: &SerialMutex = &self.inner().state;
        let owner = lock.lock_read();
        let val = unsafe { (&*self.inner().val.get()).assume_init_ref() };
        WatchGuardRef::serial(val, lock, owner)
    }

    /// Executes a closure while holding a shared guard.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        let guard = self.get();
        f(&guard)
    }

    /// Exclusive mutable access via sync
    #[inline]
    pub fn get_mut(&self) -> WatchGuardMut<'_, T> {
        let lock: &SerialMutex = &self.inner().state;
        lock.lock_write();
        let val = unsafe { (&mut *self.inner().val.get()).assume_init_mut() };
        WatchGuardMut::serial(val, lock)
    }

    /// Executes a closure while holding an exclusive guard.
    pub fn with_mut<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let mut guard = self.get_mut();
        f(&mut guard)
    }

    /// Returns a raw pointer to the inner value
    #[inline]
    pub fn as_ptr(&self) -> *mut T {
        self.inner().val.get().cast::<T>()
    }

    /// Consumes the AtomicCell and returns the inner value.
    ///
    /// # Panics
    /// Panics if there are other clones of this AtomicCell still alive.
    pub fn into_inner(self) -> T {
        // Check that we're the sole owner
        let ref_count = self.inner().ref_count.load(Ordering::Acquire);
        assert_eq!(
            ref_count, 1,
            "cannot call into_inner with multiple references"
        );

        // Get the raw pointer and prevent AtomicCell::drop from running
        let ptr = self.ptr as *mut Inner<T>;
        mem::forget(self);

        // Take ownership of the boxed inner
        let boxed = unsafe { Box::from_raw(ptr) };

        // Read the value out. MaybeUninit doesn't drop its contents,
        // so when boxed is dropped, only the Inner struct is deallocated.
        let value = unsafe { boxed.val.get().read().assume_init() };

        // boxed drops here, deallocating the Inner memory
        value
    }

    /// Atomic update with exclusive closure
    pub fn update<F>(&self, mut f: F)
    where
        F: FnMut(&mut T),
    {
        let mut guard = self.get_mut();
        f(&mut *guard);
    }

    /// Swap the value, returning the old one
    pub fn swap(&self, val: T) -> T {
        let mut guard = self.get_mut();
        mem::replace(&mut *guard, val)
    }

    /// Store a new value, dropping the old one
    pub fn store(&self, val: T) {
        drop(self.swap(val));
    }
}

impl<T: Copy + Eq> AtomicCell<T> {
    /// Compare-and-swap: if current == stored, write new and return Ok(old).
    /// Otherwise return Err with the guard still held.
    pub fn compare_exchange(&self, current: T, new: T) -> Result<T, WatchGuardMut<'_, T>> {
        let mut guard = self.get_mut();
        if *guard == current {
            let old = mem::replace(&mut *guard, new);
            Ok(old)
        } else {
            Err(guard)
        }
    }
}

impl<T> Clone for AtomicCell<T> {
    fn clone(&self) -> Self {
        crate::core::increment_ref_count(&self.inner().ref_count);
        Self { ptr: self.ptr }
    }
}

impl<T> Drop for AtomicCell<T> {
    fn drop(&mut self) {
        let inner = unsafe { &*self.ptr };
        if inner.ref_count.fetch_sub(1, Ordering::Release) == 1 {
            // Ensure memory ordering before destruction
            atomic::fence(Ordering::Acquire);

            // SAFETY: We're the last reference, so we can safely drop the inner value
            unsafe {
                // Own the allocation first, so unwinding a T destructor
                // still deallocates Inner and its SerialMutex.
                let boxed = Box::from_raw(self.ptr.cast_mut());
                (*boxed.val.get()).assume_init_drop();
            }
        }
    }
}
