use crate::sync::{Mutex, WatchGuardMut, WatchGuardRef};
use std::cell::UnsafeCell;
use std::fmt::{Debug, Formatter};
use std::marker::PhantomData;

/// A thread-safe container with exclusive & shared locks.
pub struct RwLock<T> {
    mutex: Mutex,
    data: UnsafeCell<T>,
    marker: PhantomData<T>,
}

// MutexCell is Send/Sync if T is Send/Sync
unsafe impl<T: Send> Send for RwLock<T> {}
unsafe impl<T: Send + Sync> Sync for RwLock<T> {}

impl<T> RwLock<T> {
    /// Creates a new MutexCell
    pub fn new(data: T) -> Self {
        Self {
            mutex: Mutex::new(),
            data: UnsafeCell::new(data),
            marker: PhantomData,
        }
    }

    /// Acquire exclusive lock
    pub fn lock(&self) -> WatchGuardMut<'_, T> {
        self.mutex.lock_exclusive();
        WatchGuardMut::new(self.data.get(), self.mutex.clone())
    }

    /// Try acquiring exclusive lock
    pub fn try_lock(&self) -> Option<WatchGuardMut<'_, T>> {
        if self.mutex.try_lock_exclusive() {
            Some(WatchGuardMut::new(self.data.get(), self.mutex.clone()))
        } else {
            None
        }
    }

    /// Acquire shared lock
    pub fn lock_shared(&self) -> WatchGuardRef<'_, T> {
        self.mutex.lock_shared();
        WatchGuardRef::new(unsafe { &*self.data.get() }, self.mutex.clone())
    }

    /// Try acquiring shared lock
    pub fn try_lock_shared(&self) -> Option<WatchGuardRef<'_, T>> {
        if self.mutex.try_lock_shared() {
            Some(WatchGuardRef::new(
                unsafe { &*self.data.get() },
                self.mutex.clone(),
            ))
        } else {
            None
        }
    }
}

impl<T: Debug> Debug for RwLock<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let locked = self.try_lock();
        f.debug_struct("MutexCell").field("data", &locked).finish()
    }
}
