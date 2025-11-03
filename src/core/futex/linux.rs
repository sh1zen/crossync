use core::sync::atomic::AtomicUsize;
use std::sync::atomic::AtomicU32;

#[inline]
pub fn wait(a: &AtomicUsize, expected: usize) {
    let ptr: *const AtomicU32 = (a as *const AtomicUsize) as *const AtomicU32;
    let expected_ptr: *const u32 = &(expected as u32);

    unsafe {
        libc::syscall(
            libc::SYS_futex,
            ptr,
            libc::FUTEX_WAIT | libc::FUTEX_PRIVATE_FLAG,
            expected_ptr,
            0u32,
        );
    };
}

#[inline]
pub fn wake_one(ptr: *const AtomicUsize) {
    unsafe {
        libc::syscall(
            libc::SYS_futex,
            ptr as *const AtomicU32,
            libc::FUTEX_WAKE | libc::FUTEX_PRIVATE_FLAG,
            1u32,
        );
    };
}

#[inline]
pub fn wake_all(ptr: *const AtomicUsize) {
    unsafe {
        libc::syscall(
            libc::SYS_futex,
            ptr as *const AtomicU32,
            libc::FUTEX_WAKE | libc::FUTEX_PRIVATE_FLAG,
            u32::MAX,
        );
    };
}
