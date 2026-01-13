use crate::sync::RawMutex;
use std::any::{Any, TypeId};
use std::fmt::{Debug, Formatter};
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::ptr;

/// used as wrapper for a pointer to a reference
#[must_use = "if unused the Mutex will immediately unlock"]
pub struct WatchGuardRef<'a, T: ?Sized> {
    data: *const T,
    lock: RawMutex,
    marker: PhantomData<&'a T>,
}

unsafe impl<T: ?Sized + Sync> Sync for WatchGuardRef<'_, T> {}
unsafe impl<T: ?Sized + Send> Send for WatchGuardRef<'_, T> {}

impl<'mutex, T: ?Sized> WatchGuardRef<'mutex, T> {
    ///create a new WatchGuard from a &mut T and AnyRef
    pub(crate) fn new(ptr: &'mutex T, lock: RawMutex) -> WatchGuardRef<'mutex, T> {
        Self {
            data: ptr,
            lock,
            marker: PhantomData,
        }
    }

    pub fn is_locked(&self) -> bool {
        self.lock.is_locked()
    }
}

impl<'mutex, T: Sized> WatchGuardRef<'mutex, T> {
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
        this: WatchGuardRef<'mutex, T>,
    ) -> Result<WatchGuardRef<'mutex, U>, WatchGuardRef<'mutex, T>>
    where
        T: Any + 'mutex,
        U: Any + 'mutex,
    {
        let this = ManuallyDrop::new(this);
        // First, check if the stored type is a pointer to Box<dyn Any>.
        // This ensures that we are only attempting a downcast if the inner value is a Box<dyn Any>.
        if TypeId::of::<*const Box<dyn Any>>() != this.data.type_id() {
            return Err(ManuallyDrop::into_inner(this));
        }
        // SAFETY: At this point, we assume that `self.data` is actually a `*const Box<dyn Any>`.
        // Dereferencing a raw pointer is unsafe, so the caller must guarantee the invariant:
        // The underlying data was originally created as `Box<dyn Any>`.
        let data = this.data as *const Box<dyn Any>;

        // SAFETY: Now we dereference the Box to access the inner value.
        // This is critically unsafe and works only if `data` truly points to a valid `Box<dyn Any>`.
        let data = unsafe { &**data };

        if TypeId::of::<U>() == data.type_id() {
            // Move the lock out of the ManuallyDrop (so we don't leak/unlock twice).
            // This performs a bitwise move of `this.lock`.
            let lock = unsafe { ptr::read(&this.lock) };

            // SAFETY: We are casting from `dyn Any` to `U`.
            // Safe only because we just checked that the type IDs match.
            let data = unsafe { &*(data as *const dyn Any as *const U) };

            Ok(WatchGuardRef::new(data, lock))
        } else {
            Err(ManuallyDrop::into_inner(this))
        }
    }
}

impl<T: ?Sized> Deref for WatchGuardRef<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        debug_assert!(self.lock.is_locked_shared(), "{:?}", self.lock);
        unsafe { &*self.data }
    }
}

impl<T: ?Sized> Drop for WatchGuardRef<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.unlock_shared();
    }
}

impl<'a, T, U> PartialEq<U> for WatchGuardRef<'a, T>
where
    T: PartialEq<U> + ?Sized,
{
    fn eq(&self, other: &U) -> bool {
        unsafe { &*self.data == other }
    }
}

impl<'a, T: Debug> Debug for WatchGuardRef<'a, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatchGuardRef")
            .field("data", &self.data)
            .field("lock", &self.lock)
            .finish()
    }
}
