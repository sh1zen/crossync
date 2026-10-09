use std::sync::atomic::AtomicU32;
use windows_sys::Win32::System::Threading::INFINITE;
use windows_sys::Win32::System::Threading::{WaitOnAddress, WakeByAddressAll, WakeByAddressSingle};

#[inline]
pub(crate) fn wait(a: &AtomicU32, expected: u32) {
    let ptr: *const AtomicU32 = a;
    let expected_ptr: *const u32 = &expected;
    unsafe { WaitOnAddress(ptr.cast(), expected_ptr.cast(), size_of::<u32>(), INFINITE) };
}

#[inline]
pub(crate) fn wake_one(ptr: *const AtomicU32) {
    unsafe { WakeByAddressSingle(ptr.cast()) };
}

#[inline]
pub(crate) fn wake_all(ptr: *const AtomicU32) {
    unsafe { WakeByAddressAll(ptr.cast()) };
}
