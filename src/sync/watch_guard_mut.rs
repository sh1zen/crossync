use super::guard_lock::GuardLock;
use crate::core::serial::SerialMutex;
use crate::sync::RawMutex;
use std::any::{Any, TypeId};
use std::fmt::{Debug, Formatter};
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};

/// used as wrapper for a pointer to a reference
#[must_use = "if unused the Mutex will immediately unlock"]
pub struct WatchGuardMut<'a, T: ?Sized> {
    data: *mut T,
    lock: GuardLock,
    protection: *const RawMutex,
    marker: PhantomData<&'a mut T>,
}

unsafe impl<T: ?Sized + Sync> Sync for WatchGuardMut<'_, T> {}
unsafe impl<T: ?Sized + Send> Send for WatchGuardMut<'_, T> {}

impl<'mutex, T: ?Sized> WatchGuardMut<'mutex, T> {
    ///create a new WatchGuard from a &mut T and AnyRef
    pub(crate) fn new(ptr: *mut T, lock: *const RawMutex) -> WatchGuardMut<'mutex, T> {
        Self {
            data: ptr,
            lock: GuardLock::exclusive(lock),
            protection: std::ptr::null(),
            marker: PhantomData,
        }
    }

    #[inline]
    pub(crate) fn serial(ptr: *mut T, lock: *const SerialMutex) -> Self {
        Self {
            data: ptr,
            lock: GuardLock::serial_write(lock),
            protection: std::ptr::null(),
            marker: PhantomData,
        }
    }

    #[inline]
    pub(crate) fn protect(mut self, lock: *const RawMutex) -> Self {
        self.protection = lock;
        self
    }

    pub fn is_locked(&self) -> bool {
        unsafe { self.lock.is_locked() }
    }
}

impl<'mutex, T: Sized> WatchGuardMut<'mutex, T> {
    /// Attempts to downcast the inner value to a specific type `U`.
    ///
    /// # Safety
    /// This function is **highly unsafe** and should only be used under very specific circumstances:
    /// 1. The `WatchGuard` must have been constructed over a `Box::new(pointer)` stored as `Box<dyn Any>`.
    /// 2. The type stored inside the `Box` must actually match the requested type `U`.
    /// 3. Violating these assumptions can lead to undefined behavior, including invalid memory access or data corruption.
    ///
    /// # Usage Notes
    /// - This is **not a general-purpose downcast**. It only works when the inner data is a `Box<dyn Any>`
    ///   because it relies on raw pointer dereferencing to access the inner value.
    /// - `downcast_ref` returns `Some(WatchGuardRef<'mutex, U>)` if the type matches, otherwise `None`.
    /// - The caller must ensure that the original `Box<dyn Any>` is valid for the lifetime `'mutex`.
    /// - Cloning of the internal lock is done to maintain guard semantics safely.
    ///
    /// # Usage case
    /// - To decapsulate an internal value stored as `Box<dyn Any>` and access it as a concrete type.
    /// - Safely preserves lock/guard semantics while exposing the inner value.
    pub unsafe fn downcast<U: Any>(
        this: WatchGuardMut<'mutex, T>,
    ) -> Result<WatchGuardMut<'mutex, U>, WatchGuardMut<'mutex, T>>
    where
        T: Any + 'mutex,
        U: Any + 'mutex,
    {
        let this = ManuallyDrop::new(this);
        // First, check if the stored type is a pointer to Box<dyn Any>.
        // This ensures that we are only attempting a downcast if the inner value is a Box<dyn Any>.
        if TypeId::of::<*mut Box<dyn Any>>() != this.data.type_id() {
            return Err(ManuallyDrop::into_inner(this));
        }
        // SAFETY: At this point, we assume that `self.data` is actually a `*const Box<dyn Any>`.
        // Dereferencing a raw pointer is unsafe, so the caller must guarantee the invariant:
        // The underlying data was originally created as `Box<dyn Any>`.
        let data = this.data as *mut Box<dyn Any>;

        // SAFETY: Now we dereference the Box to access the inner value.
        // This is critically unsafe and works only if `data` truly points to a valid `Box<dyn Any>`.
        let dyn_any_ref: &mut dyn Any = unsafe { &mut **data };

        if TypeId::of::<U>() == (&*dyn_any_ref).type_id() {
            // SAFETY: We are casting from `dyn Any` to `U`.
            // Safe only because we just checked that the type IDs match.
            let u_ptr = unsafe { &mut *(dyn_any_ref as *mut dyn Any as *mut U) };

            Ok(WatchGuardMut {
                data: u_ptr,
                lock: this.lock,
                protection: this.protection,
                marker: PhantomData,
            })
        } else {
            Err(ManuallyDrop::into_inner(this))
        }
    }
}

impl<T: ?Sized> Deref for WatchGuardMut<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        debug_assert!(unsafe { self.lock.is_locked() });
        unsafe { &*self.data }
    }
}

impl<T: ?Sized> DerefMut for WatchGuardMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        debug_assert!(unsafe { self.lock.is_locked() });
        unsafe { &mut *self.data }
    }
}

impl<T: ?Sized> Drop for WatchGuardMut<'_, T> {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            self.lock.unlock();
            if !self.protection.is_null() {
                (*self.protection).unlock_shared();
            }
        }
    }
}

impl<'a, T, U> PartialEq<U> for WatchGuardMut<'a, T>
where
    T: PartialEq<U> + ?Sized,
{
    fn eq(&self, other: &U) -> bool {
        unsafe { &*self.data == other }
    }
}

impl<'a, T: Debug> Debug for WatchGuardMut<'a, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatchGuardMut")
            .field("data", &self.data)
            .field("lock", &self.lock)
            .finish()
    }
}
