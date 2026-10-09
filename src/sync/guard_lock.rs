use crate::core::serial::SerialMutex;
use crate::sync::RawMutex;

#[derive(Clone, Copy, Debug)]
enum Kind {
    Shared,
    Exclusive,
    SerialRead,
    SerialWrite,
}

// Pack the reader token into the discriminant's padding on 64-bit targets.
#[derive(Clone, Copy, Debug)]
pub(super) struct GuardLock {
    lock: *const (),
    owner: u32,
    kind: Kind,
}

impl GuardLock {
    #[inline]
    pub(super) fn shared(lock: *const RawMutex) -> Self {
        Self {
            lock: lock.cast(),
            owner: 0,
            kind: Kind::Shared,
        }
    }
    #[inline]
    pub(super) fn exclusive(lock: *const RawMutex) -> Self {
        Self {
            lock: lock.cast(),
            owner: 0,
            kind: Kind::Exclusive,
        }
    }
    #[inline]
    pub(super) fn serial_read(lock: *const SerialMutex, owner: u32) -> Self {
        Self {
            lock: lock.cast(),
            owner,
            kind: Kind::SerialRead,
        }
    }
    #[inline]
    pub(super) fn serial_write(lock: *const SerialMutex) -> Self {
        Self {
            lock: lock.cast(),
            owner: 0,
            kind: Kind::SerialWrite,
        }
    }
    #[inline]
    pub(super) unsafe fn is_locked(self) -> bool {
        unsafe {
            match self.kind {
                Kind::Shared => (*self.lock.cast::<RawMutex>()).is_locked_shared(),
                Kind::Exclusive => (*self.lock.cast::<RawMutex>()).is_locked_exclusive(),
                Kind::SerialRead => (*self.lock.cast::<SerialMutex>()).is_read_locked(),
                Kind::SerialWrite => (*self.lock.cast::<SerialMutex>()).is_write_locked(),
            }
        }
    }
    #[inline]
    pub(super) unsafe fn unlock(self) {
        unsafe {
            match self.kind {
                Kind::Shared => (*self.lock.cast::<RawMutex>()).unlock_shared(),
                Kind::Exclusive => (*self.lock.cast::<RawMutex>()).unlock_exclusive(),
                Kind::SerialRead => (*self.lock.cast::<SerialMutex>()).unlock_read(self.owner),
                Kind::SerialWrite => (*self.lock.cast::<SerialMutex>()).unlock_write(),
            }
        }
    }
}
