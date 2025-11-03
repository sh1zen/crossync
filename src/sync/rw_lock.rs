use crate::sync::{RawMutex, WatchGuardMut, WatchGuardRef};
use crossbeam_utils::CachePadded;
use std::cell::UnsafeCell;
use std::fmt::{Debug, Formatter};
use std::sync::atomic;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A thread-safe container with exclusive & shared locks.
pub(super) struct InnerRwLock<T> {
    mutex: RawMutex,
    ref_count: CachePadded<AtomicUsize>,
    data: UnsafeCell<T>,
}

impl<T> InnerRwLock<T> {
    /// Creates a new `InnerMutex` with all fields initialized.
    fn new(val: T) -> Self {
        Self {
            mutex: RawMutex::new(),
            ref_count: CachePadded::new(AtomicUsize::new(1)),
            data: UnsafeCell::new(val),
        }
    }
}

/// Un RwLock clonabile, reference-counted.
/// La clonazione non duplica i dati, ma incrementa un contatore interno.
#[repr(transparent)]
pub struct RwLock<T> {
    ptr: *const InnerRwLock<T>,
}

// MutexCell is Send/Sync if T is Send/Sync
unsafe impl<T: Send> Send for RwLock<T> {}
unsafe impl<T: Send + Sync> Sync for RwLock<T> {}

impl<T> RwLock<T> {
    /// Creates a new MutexCell
    pub fn new(val: T) -> Self {
        let ptr = Box::into_raw(Box::new(InnerRwLock::new(val)));
        Self { ptr }
    }

    /// Returns a shared reference to the internal sync data
    #[inline(always)]
    fn inner(&self) -> &InnerRwLock<T> {
        unsafe { &*self.ptr }
    }

    /// Acquire exclusive lock
    pub fn lock(&self) -> WatchGuardMut<'_, T> {
        let inner = self.inner();
        inner.mutex.lock_exclusive();
        WatchGuardMut::new(inner.data.get(), inner.mutex.clone())
    }

    /// Try acquiring exclusive lock
    pub fn try_lock(&self) -> Option<WatchGuardMut<'_, T>> {
        let inner = self.inner();
        if inner.mutex.try_lock_exclusive() {
            Some(WatchGuardMut::new(inner.data.get(), inner.mutex.clone()))
        } else {
            None
        }
    }

    /// Acquire shared lock
    pub fn lock_shared(&self) -> WatchGuardRef<'_, T> {
        let inner = self.inner();
        inner.mutex.lock_shared();
        WatchGuardRef::new(unsafe { &*inner.data.get() }, inner.mutex.clone())
    }

    /// Try acquiring shared lock
    pub fn try_lock_shared(&self) -> Option<WatchGuardRef<'_, T>> {
        let inner = self.inner();
        if inner.mutex.try_lock_shared() {
            Some(WatchGuardRef::new(
                unsafe { &*inner.data.get() },
                inner.mutex.clone(),
            ))
        } else {
            None
        }
    }
}

impl<T> Clone for RwLock<T> {
    fn clone(&self) -> Self {
        self.inner().ref_count.fetch_add(1, Ordering::Relaxed);
        RwLock { ptr: self.ptr }
    }
}

impl<T> Drop for RwLock<T> {
    fn drop(&mut self) {
        if self.inner().ref_count.fetch_sub(1, Ordering::Release) == 1 {
            atomic::fence(Ordering::Acquire);
            let ptr = self.ptr as *mut InnerRwLock<T>;
            unsafe { drop(Box::from_raw(ptr)) };
        }
    }
}

impl<T: Debug> Debug for RwLock<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let locked = self.try_lock();
        f.debug_struct("MutexCell").field("data", &locked).finish()
    }
}