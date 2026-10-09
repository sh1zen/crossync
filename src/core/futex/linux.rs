use core::sync::atomic::AtomicU32;

#[inline]
fn word(ptr: *const AtomicU32) -> *mut u32 {
    let ptr = ptr.cast_mut().cast::<u32>();
    ptr
}

#[inline]
pub(super) fn wait(a: &AtomicU32, expected: u32) {
    let ptr = word(a);

    unsafe {
        libc::syscall(
            libc::SYS_futex,
            ptr,
            libc::FUTEX_WAIT | libc::FUTEX_PRIVATE_FLAG,
            expected as u32,
            core::ptr::null::<libc::timespec>(),
            core::ptr::null_mut::<u32>(),
            0u32,
        );
    };
}

#[inline]
pub(super) fn wake_one(ptr: *const AtomicU32) {
    unsafe {
        libc::syscall(
            libc::SYS_futex,
            word(ptr),
            libc::FUTEX_WAKE | libc::FUTEX_PRIVATE_FLAG,
            1u32,
            core::ptr::null::<libc::timespec>(),
            core::ptr::null_mut::<u32>(),
            0u32,
        );
    };
}

#[inline]
pub(super) fn wake_all(ptr: *const AtomicU32) {
    unsafe {
        libc::syscall(
            libc::SYS_futex,
            word(ptr),
            libc::FUTEX_WAKE | libc::FUTEX_PRIVATE_FLAG,
            i32::MAX as u32,
            core::ptr::null::<libc::timespec>(),
            core::ptr::null_mut::<u32>(),
            0u32,
        );
    };
}
