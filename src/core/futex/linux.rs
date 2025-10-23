use core::sync::atomic::AtomicUsize;
use std::usize;

#[inline]
pub fn wait(a: &AtomicUsize, expected: usize) {
    let ptr: *const AtomicUsize = a;
    let expected_ptr: *const usize = &expected;

    unsafe {
        libc::syscall(
            libc::SYS_futex,
            ptr,
            libc::FUTEX_WAIT | libc::FUTEX_PRIVATE_FLAG,
            expected_ptr,
            0usize,
        );
    };
}

#[inline]
pub fn wake_one(ptr: *const AtomicUsize) {
    unsafe {
        libc::syscall(
            libc::SYS_futex,
            ptr,
            libc::FUTEX_WAKE | libc::FUTEX_PRIVATE_FLAG,
            1usize,
        );
    };
}

#[inline]
pub fn wake_all(ptr: *const AtomicUsize) {
    unsafe {
        libc::syscall(
            libc::SYS_futex,
            ptr,
            libc::FUTEX_WAKE | libc::FUTEX_PRIVATE_FLAG,
            usize::MAX,
        );
    };
}
