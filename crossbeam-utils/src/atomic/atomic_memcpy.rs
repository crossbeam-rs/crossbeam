use core::mem::MaybeUninit;
#[cfg(crossbeam_sanitize_thread)]
use core::{
    mem, slice,
    sync::atomic::{AtomicU8, Ordering},
};

pub(crate) unsafe fn load<T>(src: *const T) -> MaybeUninit<T> {
    atomic_maybe_uninit::cfg_has_atomic_memcpy! {
        use core::sync::atomic::Ordering;
        use atomic_maybe_uninit::PerByteAtomicMaybeUninit;
        if cfg!(not(any(miri, crossbeam_sanitize_any))) {
            unsafe {
                (*src.cast::<PerByteAtomicMaybeUninit<T>>()).load(Ordering::Relaxed)
            }
        } else {
            unsafe { load_fallback(src) }
        }
    }
    atomic_maybe_uninit::cfg_no_atomic_memcpy! {
        unsafe { load_fallback(src) }
    }
}
unsafe fn load_fallback<T>(src: *const T) -> MaybeUninit<T> {
    #[cfg(not(crossbeam_sanitize_thread))]
    // We need a volatile read here because other threads might concurrently modify the
    // value. In theory, data races are *always* UB, even if we use volatile reads and
    // discard the data when a data race is detected. The proper solution would be to
    // do atomic reads and atomic writes like the code above, but inline assembly is
    // not stable on some tier 2/3 targets and unsupported in Miri/Sanitizer. Hence,
    // as a hack, we use a volatile write instead.
    // See also https://github.com/rust-lang/rfcs/pull/3301.
    // Load as `MaybeUninit` because we may load a value that is not valid as `T`.
    unsafe {
        src.cast::<MaybeUninit<T>>().read_volatile()
    }
    #[cfg(crossbeam_sanitize_thread)]
    // TODO: comment
    unsafe {
        let mut out: MaybeUninit<T> = MaybeUninit::uninit();
        for (src, out) in slice::from_raw_parts(src.cast::<AtomicU8>(), mem::size_of::<T>())
            .iter()
            .zip(slice::from_raw_parts_mut(
                out.as_mut_ptr().cast::<MaybeUninit<u8>>(),
                mem::size_of::<T>(),
            ))
        {
            *out = mem::transmute::<u8, MaybeUninit<u8>>(src.load(Ordering::Relaxed));
        }
        out
    }
}

pub(crate) unsafe fn store<T>(dst: *mut T, val: MaybeUninit<T>) {
    atomic_maybe_uninit::cfg_has_atomic_memcpy! {
        use core::sync::atomic::Ordering;
        use atomic_maybe_uninit::PerByteAtomicMaybeUninit;
        if cfg!(not(any(miri, crossbeam_sanitize_any))) {
            unsafe {
                (*dst.cast::<PerByteAtomicMaybeUninit<T>>()).store(val, Ordering::Relaxed);
            }
            return;
        }
    }
    unsafe { store_fallback(dst, val) }
}
unsafe fn store_fallback<T>(dst: *mut T, val: MaybeUninit<T>) {
    #[cfg(not(crossbeam_sanitize_thread))]
    // This method might be concurrently called with another `read` at the same index, which is
    // technically speaking a data race and therefore UB. We should use an atomic store here
    // like the code above, but inline assembly is not stable on some tier 2/3 targets and
    // unsupported in Miri/Sanitizer. Hence, as a hack, we use a volatile write instead.
    // See also https://github.com/rust-lang/rfcs/pull/3301.
    unsafe {
        dst.cast::<MaybeUninit<T>>().write_volatile(val);
    }
    #[cfg(crossbeam_sanitize_thread)]
    // TODO: comment
    unsafe {
        for (dst, val) in slice::from_raw_parts(dst.cast::<AtomicU8>(), mem::size_of::<T>())
            .iter()
            .zip(slice::from_raw_parts(
                val.as_ptr().cast::<MaybeUninit<u8>>(),
                mem::size_of::<T>(),
            ))
        {
            dst.store(
                mem::transmute::<MaybeUninit<u8>, u8>(*val),
                Ordering::Relaxed,
            );
        }
    }
}
