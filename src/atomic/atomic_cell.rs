use core::cell::UnsafeCell;
use core::fmt;
use core::mem;
use core::ptr;
use core::slice;
use core::sync::atomic::{self, AtomicBool, Ordering};

/// A thread-safe mutable memory location.
///
/// This type is equivalent to [`Cell`], except it can also be shared among multiple threads.
///
/// Operations on `AtomicCell`s use atomic instructions whenever possible, and synchronize using
/// global locks otherwise. You can call [`AtomicCell::<T>::is_lock_free()`] to check whether
/// atomic instructions or locks will be used.
///
/// [`Cell`]: https://doc.rust-lang.org/std/cell/struct.Cell.html
/// [`AtomicCell::<T>::is_lock_free()`]: struct.AtomicCell.html#method.is_lock_free
pub struct AtomicCell<T> {
    /// The inner value.
    ///
    /// If this value can be transmuted into a primitive atomic type, it will be treated as such.
    /// Otherwise, all potentially concurrent operations on this data will be protected by a global
    /// lock.
    value: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for AtomicCell<T> {}
unsafe impl<T: Send> Sync for AtomicCell<T> {}

impl<T> AtomicCell<T> {
    /// Creates a new atomic cell initialized with `val`.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::atomic::AtomicCell;
    ///
    /// let a = AtomicCell::new(7);
    /// ```
    pub fn new(val: T) -> AtomicCell<T> {
        AtomicCell {
            value: UnsafeCell::new(val),
        }
    }

    /// Returns a mutable reference to the inner value.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::atomic::AtomicCell;
    ///
    /// let mut a = AtomicCell::new(7);
    /// *a.get_mut() += 1;
    ///
    /// assert_eq!(a.load(), 8);
    /// ```
    pub fn get_mut(&mut self) -> &mut T {
        unsafe { &mut *self.value.get() }
    }

    /// Unwraps the atomic cell and returns its inner value.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::atomic::AtomicCell;
    ///
    /// let mut a = AtomicCell::new(7);
    /// let v = a.into_inner();
    ///
    /// assert_eq!(v, 7);
    /// ```
    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }

    /// Returns `true` if operations on values of this type are lock-free.
    ///
    /// If the compiler or the platform doesn't support the necessary atomic instructions,
    /// `AtomicCell<T>` will use global locks for every potentially concurrent atomic operation.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::atomic::AtomicCell;
    ///
    /// // This type is internally represented as `AtomicUsize` so we can just use atomic
    /// // operations provided by it.
    /// assert_eq!(AtomicCell::<usize>::is_lock_free(), true);
    ///
    /// // A wrapper struct around `isize`.
    /// struct Foo {
    ///     bar: isize,
    /// }
    /// // `AtomicCell<Foo>` will be internally represented as `AtomicIsize`.
    /// assert_eq!(AtomicCell::<Foo>::is_lock_free(), true);
    ///
    /// // Operations on zero-sized types are always lock-free.
    /// assert_eq!(AtomicCell::<()>::is_lock_free(), true);
    ///
    /// // Very large types cannot be represented as any of the standard atomic types, so atomic
    /// // operations on them will have to use global locks for synchronization.
    /// assert_eq!(AtomicCell::<[u8; 1000]>::is_lock_free(), false);
    /// ```
    pub fn is_lock_free() -> bool {
        atomic_is_lock_free::<T>()
    }

    /// Stores `val` into the atomic cell.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::atomic::AtomicCell;
    ///
    /// let a = AtomicCell::new(7);
    ///
    /// assert_eq!(a.load(), 7);
    /// a.store(8);
    /// assert_eq!(a.load(), 8);
    /// ```
    pub fn store(&self, val: T) {
        if mem::needs_drop::<T>() {
            drop(self.swap(val));
        } else {
            unsafe {
                atomic_store(self.value.get(), val);
            }
        }
    }

    /// Stores `val` into the atomic cell and returns the previous value.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::atomic::AtomicCell;
    ///
    /// let a = AtomicCell::new(7);
    ///
    /// assert_eq!(a.load(), 7);
    /// assert_eq!(a.swap(8), 7);
    /// assert_eq!(a.load(), 8);
    /// ```
    pub fn swap(&self, val: T) -> T {
        unsafe { atomic_swap(self.value.get(), val) }
    }
}

impl<T: Copy> AtomicCell<T> {
    /// Loads a value.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::atomic::AtomicCell;
    ///
    /// let a = AtomicCell::new(7);
    ///
    /// assert_eq!(a.load(), 7);
    /// ```
    pub fn load(&self) -> T {
        unsafe { atomic_load(self.value.get()) }
    }
}

impl<T: Copy + Eq> AtomicCell<T> {
    /// If the current value equals `current`, stores `new` into the atomic cell.
    ///
    /// The return value is always the previous value. If it is equal to `current`, then the value
    /// was updated.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::atomic::AtomicCell;
    ///
    /// let a = AtomicCell::new(1);
    ///
    /// assert_eq!(a.compare_exchange(2, 3), Err(1));
    /// assert_eq!(a.load(), 1);
    ///
    /// assert_eq!(a.compare_exchange(1, 2), Ok(1));
    /// assert_eq!(a.load(), 2);
    /// ```
    pub fn compare_and_swap(&self, current: T, new: T) -> T {
        match self.compare_exchange(current, new) {
            Ok(v) => v,
            Err(v) => v,
        }
    }

    /// If the current value equals `current`, stores `new` into the atomic cell.
    ///
    /// The return value is a result indicating whether the new value was written and containing
    /// the previous value. On success this value is guaranteed to be equal to `current`.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::atomic::AtomicCell;
    ///
    /// let a = AtomicCell::new(1);
    ///
    /// assert_eq!(a.compare_exchange(2, 3), Err(1));
    /// assert_eq!(a.load(), 1);
    ///
    /// assert_eq!(a.compare_exchange(1, 2), Ok(1));
    /// assert_eq!(a.load(), 2);
    /// ```
    pub fn compare_exchange(&self, mut current: T, new: T) -> Result<T, T> {
        loop {
            match unsafe { atomic_compare_exchange_weak(self.value.get(), current, new) } {
                Ok(_) => return Ok(current),
                Err(previous) => {
                    if previous != current {
                        return Err(previous);
                    }

                    // The compare-exchange operation has failed and didn't store `new`. The
                    // failure is either spurious, or `previous` was semantically equal to
                    // `current` but not byte-equal. Let's retry with `previous` as the new
                    // `current`.
                    current = previous;
                }
            }
        }
    }
}

macro_rules! impl_arithmetic {
    ($t:ty, $example:tt) => {
        impl AtomicCell<$t> {
            /// Increments the current value by `val` and returns the previous value.
            ///
            /// The addition wraps on overflow.
            ///
            /// # Examples
            ///
            /// ```
            /// use crossbeam_utils::atomic::AtomicCell;
            ///
            #[doc = $example]
            ///
            /// assert_eq!(a.fetch_add(3), 7);
            /// assert_eq!(a.load(), 10);
            /// ```
            #[inline]
            pub fn fetch_add(&self, val: $t) -> $t {
                if can_transmute::<$t, atomic::AtomicUsize>() {
                    let a = unsafe { &*(self.value.get() as *const atomic::AtomicUsize) };
                    a.fetch_add(val as usize, Ordering::SeqCst) as $t
                } else {
                    let _lock = lock(self.value.get() as usize);
                    let value = unsafe { &mut *(self.value.get()) };
                    let old = *value;
                    *value = value.wrapping_add(val);
                    old
                }
            }

            /// Decrements the current value by `val` and returns the previous value.
            ///
            /// The subtraction wraps on overflow.
            ///
            /// # Examples
            ///
            /// ```
            /// use crossbeam_utils::atomic::AtomicCell;
            ///
            #[doc = $example]
            ///
            /// assert_eq!(a.fetch_sub(3), 7);
            /// assert_eq!(a.load(), 4);
            /// ```
            #[inline]
            pub fn fetch_sub(&self, val: $t) -> $t {
                if can_transmute::<$t, atomic::AtomicUsize>() {
                    let a = unsafe { &*(self.value.get() as *const atomic::AtomicUsize) };
                    a.fetch_sub(val as usize, Ordering::SeqCst) as $t
                } else {
                    let _lock = lock(self.value.get() as usize);
                    let value = unsafe { &mut *(self.value.get()) };
                    let old = *value;
                    *value = value.wrapping_sub(val);
                    old
                }
            }

            /// Applies bitwise "and" to the current value and returns the previous value.
            ///
            /// # Examples
            ///
            /// ```
            /// use crossbeam_utils::atomic::AtomicCell;
            ///
            #[doc = $example]
            ///
            /// assert_eq!(a.fetch_and(3), 7);
            /// assert_eq!(a.load(), 3);
            /// ```
            #[inline]
            pub fn fetch_and(&self, val: $t) -> $t {
                if can_transmute::<$t, atomic::AtomicUsize>() {
                    let a = unsafe { &*(self.value.get() as *const atomic::AtomicUsize) };
                    a.fetch_and(val as usize, Ordering::SeqCst) as $t
                } else {
                    let _lock = lock(self.value.get() as usize);
                    let value = unsafe { &mut *(self.value.get()) };
                    let old = *value;
                    *value = *value & val;
                    old
                }
            }

            /// Applies bitwise "or" to the current value and returns the previous value.
            ///
            /// # Examples
            ///
            /// ```
            /// use crossbeam_utils::atomic::AtomicCell;
            ///
            #[doc = $example]
            ///
            /// assert_eq!(a.fetch_or(16), 7);
            /// assert_eq!(a.load(), 23);
            /// ```
            #[inline]
            pub fn fetch_or(&self, val: $t) -> $t {
                if can_transmute::<$t, atomic::AtomicUsize>() {
                    let a = unsafe { &*(self.value.get() as *const atomic::AtomicUsize) };
                    a.fetch_or(val as usize, Ordering::SeqCst) as $t
                } else {
                    let _lock = lock(self.value.get() as usize);
                    let value = unsafe { &mut *(self.value.get()) };
                    let old = *value;
                    *value = *value | val;
                    old
                }
            }

            /// Applies bitwise "xor" to the current value and returns the previous value.
            ///
            /// # Examples
            ///
            /// ```
            /// use crossbeam_utils::atomic::AtomicCell;
            ///
            #[doc = $example]
            ///
            /// assert_eq!(a.fetch_xor(2), 7);
            /// assert_eq!(a.load(), 5);
            /// ```
            #[inline]
            pub fn fetch_xor(&self, val: $t) -> $t {
                if can_transmute::<$t, atomic::AtomicUsize>() {
                    let a = unsafe { &*(self.value.get() as *const atomic::AtomicUsize) };
                    a.fetch_xor(val as usize, Ordering::SeqCst) as $t
                } else {
                    let _lock = lock(self.value.get() as usize);
                    let value = unsafe { &mut *(self.value.get()) };
                    let old = *value;
                    *value = *value ^ val;
                    old
                }
            }
        }
    };
    ($t:ty, $atomic:ty, $example:tt) => {
        impl AtomicCell<$t> {
            /// Increments the current value by `val` and returns the previous value.
            ///
            /// The addition wraps on overflow.
            ///
            /// # Examples
            ///
            /// ```
            /// use crossbeam_utils::atomic::AtomicCell;
            ///
            #[doc = $example]
            ///
            /// assert_eq!(a.fetch_add(3), 7);
            /// assert_eq!(a.load(), 10);
            /// ```
            #[inline]
            pub fn fetch_add(&self, val: $t) -> $t {
                let a = unsafe { &*(self.value.get() as *const $atomic) };
                a.fetch_add(val, Ordering::SeqCst)
            }

            /// Decrements the current value by `val` and returns the previous value.
            ///
            /// The subtraction wraps on overflow.
            ///
            /// # Examples
            ///
            /// ```
            /// use crossbeam_utils::atomic::AtomicCell;
            ///
            #[doc = $example]
            ///
            /// assert_eq!(a.fetch_sub(3), 7);
            /// assert_eq!(a.load(), 4);
            /// ```
            #[inline]
            pub fn fetch_sub(&self, val: $t) -> $t {
                let a = unsafe { &*(self.value.get() as *const $atomic) };
                a.fetch_sub(val, Ordering::SeqCst)
            }

            /// Applies bitwise "and" to the current value and returns the previous value.
            ///
            /// # Examples
            ///
            /// ```
            /// use crossbeam_utils::atomic::AtomicCell;
            ///
            #[doc = $example]
            ///
            /// assert_eq!(a.fetch_and(3), 7);
            /// assert_eq!(a.load(), 3);
            /// ```
            #[inline]
            pub fn fetch_and(&self, val: $t) -> $t {
                let a = unsafe { &*(self.value.get() as *const $atomic) };
                a.fetch_and(val, Ordering::SeqCst)
            }

            /// Applies bitwise "or" to the current value and returns the previous value.
            ///
            /// # Examples
            ///
            /// ```
            /// use crossbeam_utils::atomic::AtomicCell;
            ///
            #[doc = $example]
            ///
            /// assert_eq!(a.fetch_or(16), 7);
            /// assert_eq!(a.load(), 23);
            /// ```
            #[inline]
            pub fn fetch_or(&self, val: $t) -> $t {
                let a = unsafe { &*(self.value.get() as *const $atomic) };
                a.fetch_or(val, Ordering::SeqCst)
            }

            /// Applies bitwise "xor" to the current value and returns the previous value.
            ///
            /// # Examples
            ///
            /// ```
            /// use crossbeam_utils::atomic::AtomicCell;
            ///
            #[doc = $example]
            ///
            /// assert_eq!(a.fetch_xor(2), 7);
            /// assert_eq!(a.load(), 5);
            /// ```
            #[inline]
            pub fn fetch_xor(&self, val: $t) -> $t {
                let a = unsafe { &*(self.value.get() as *const $atomic) };
                a.fetch_xor(val, Ordering::SeqCst)
            }
        }
    };
}

cfg_if! {
    if #[cfg(feature = "nightly")] {
        impl_arithmetic!(u8, atomic::AtomicU8, "let a = AtomicCell::new(7u8);");
        impl_arithmetic!(i8, atomic::AtomicI8, "let a = AtomicCell::new(7i8);");
        impl_arithmetic!(u16, atomic::AtomicU16, "let a = AtomicCell::new(7u16);");
        impl_arithmetic!(i16, atomic::AtomicI16, "let a = AtomicCell::new(7i16);");
        impl_arithmetic!(u32, atomic::AtomicU32, "let a = AtomicCell::new(7u32);");
        impl_arithmetic!(i32, atomic::AtomicI32, "let a = AtomicCell::new(7i32);");
        impl_arithmetic!(u64, atomic::AtomicU64, "let a = AtomicCell::new(7u64);");
        impl_arithmetic!(i64, atomic::AtomicI64, "let a = AtomicCell::new(7i64);");
    } else {
        impl_arithmetic!(u8, "let a = AtomicCell::new(7u8);");
        impl_arithmetic!(i8, "let a = AtomicCell::new(7i8);");
        impl_arithmetic!(u16, "let a = AtomicCell::new(7u16);");
        impl_arithmetic!(i16, "let a = AtomicCell::new(7i16);");
        impl_arithmetic!(u32, "let a = AtomicCell::new(7u32);");
        impl_arithmetic!(i32, "let a = AtomicCell::new(7i32);");
        impl_arithmetic!(u64, "let a = AtomicCell::new(7u64);");
        impl_arithmetic!(i64, "let a = AtomicCell::new(7i64);");
    }
}

impl_arithmetic!(usize, atomic::AtomicUsize, "let a = AtomicCell::new(7usize);");
impl_arithmetic!(isize, atomic::AtomicIsize, "let a = AtomicCell::new(7isize);");

impl AtomicCell<bool> {
    /// Applies logical "and" to the current value and returns the previous value.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::atomic::AtomicCell;
    ///
    /// let a = AtomicCell::new(true);
    ///
    /// assert_eq!(a.fetch_and(true), true);
    /// assert_eq!(a.load(), true);
    ///
    /// assert_eq!(a.fetch_and(false), true);
    /// assert_eq!(a.load(), false);
    /// ```
    #[inline]
    pub fn fetch_and(&self, val: bool) -> bool {
        let a = unsafe { &*(self.value.get() as *const AtomicBool) };
        a.fetch_and(val, Ordering::SeqCst)
    }

    /// Applies logical "or" to the current value and returns the previous value.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::atomic::AtomicCell;
    ///
    /// let a = AtomicCell::new(false);
    ///
    /// assert_eq!(a.fetch_or(false), false);
    /// assert_eq!(a.load(), false);
    ///
    /// assert_eq!(a.fetch_or(true), false);
    /// assert_eq!(a.load(), true);
    /// ```
    #[inline]
    pub fn fetch_or(&self, val: bool) -> bool {
        let a = unsafe { &*(self.value.get() as *const AtomicBool) };
        a.fetch_or(val, Ordering::SeqCst)
    }

    /// Applies logical "xor" to the current value and returns the previous value.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossbeam_utils::atomic::AtomicCell;
    ///
    /// let a = AtomicCell::new(true);
    ///
    /// assert_eq!(a.fetch_xor(false), true);
    /// assert_eq!(a.load(), true);
    ///
    /// assert_eq!(a.fetch_xor(true), true);
    /// assert_eq!(a.load(), false);
    /// ```
    #[inline]
    pub fn fetch_xor(&self, val: bool) -> bool {
        let a = unsafe { &*(self.value.get() as *const AtomicBool) };
        a.fetch_xor(val, Ordering::SeqCst)
    }
}

impl<T: Default> Default for AtomicCell<T> {
    fn default() -> AtomicCell<T> {
        AtomicCell::new(T::default())
    }
}

impl<T: Copy + fmt::Debug> fmt::Debug for AtomicCell<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("AtomicCell")
            .field("value", &self.load())
            .finish()
    }
}

/// Returns `true` if the two values are equal byte-for-byte.
fn byte_eq<T>(a: &T, b: &T) -> bool {
    unsafe {
        let a = slice::from_raw_parts(a as *const _ as *const u8, mem::size_of::<T>());
        let b = slice::from_raw_parts(b as *const _ as *const u8, mem::size_of::<T>());
        a == b
    }
}

/// Returns `true` if values of type `A` can be transmuted into values of type `B`.
fn can_transmute<A, B>() -> bool {
    // Sizes must be equal, but alignment of `A` must be greater or equal than that of `B`.
    mem::size_of::<A>() == mem::size_of::<B>() && mem::align_of::<A>() >= mem::align_of::<B>()
}

/// Automatically releases a lock when dropped.
struct LockGuard {
    lock: &'static AtomicBool,
}

impl Drop for LockGuard {
    #[inline]
    fn drop(&mut self) {
        self.lock.store(false, Ordering::Release);
    }
}

/// Acquires the lock for atomic data stored at the given address.
///
/// This function is used to protect atomic data which doesn't fit into any of the primitive atomic
/// types in `std::sync::atomic`. Operations on such atomics must therefore use a global lock.
///
/// However, there is not only one global lock but an array of many locks, and one of them is
/// picked based on the given address. Having many locks reduces contention and improves
/// scalability.
#[inline]
fn lock(addr: usize) -> LockGuard {
    // The number of locks is prime.
    const LEN: usize = 499;

    const A: AtomicBool = AtomicBool::new(false);
    static LOCKS: [AtomicBool; LEN] = [
        A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A,
        A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A,
        A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A,
        A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A,
        A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A,
        A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A,
        A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A,
        A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A,
        A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A,
        A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A,
        A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A,
        A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A,
        A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A,
        A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A,
        A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A,
        A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A,
        A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A, A,
    ];

    // If the modulus is a constant number, the compiler will use crazy math to transform this into
    // a sequence of cheap arithmetic operations rather than using the slow modulo instruction.
    let lock = &LOCKS[addr % LEN];

    let mut step = 0usize;

    while lock.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        if step < 5 {
            // Just try again.
        } else if step < 10 {
            atomic::spin_loop_hint();
        } else {
            #[cfg(not(feature = "use_std"))]
            atomic::spin_loop_hint();

            #[cfg(feature = "use_std")]
            ::std::thread::yield_now();
        }
        step = step.wrapping_add(1);
    }

    LockGuard { lock }
}

/// An atomic `()`.
///
/// All operations are noops.
struct AtomicUnit;

impl AtomicUnit {
    #[inline]
    fn load(&self, _order: Ordering) {}

    #[inline]
    fn store(&self, _val: (), _order: Ordering) {}

    #[inline]
    fn swap(&self, _val: (), _order: Ordering) {}

    #[inline]
    fn compare_exchange_weak(
        &self,
        _current: (),
        _new: (),
        _success: Ordering,
        _failure: Ordering
    ) -> Result<(), ()> {
        Ok(())
    }
}

macro_rules! atomic {
    // If values of type `$t` can be transmuted into values of the primitive atomic type `$atomic`,
    // declares variable `$a` of type `$atomic` and executes `$atomic_op`, breaking out of the loop.
    (@check, $t:ty, $atomic:ty, $a:ident, $atomic_op:expr) => {
        if can_transmute::<$t, $atomic>() {
            let $a: &$atomic;
            break $atomic_op
        }
    };

    // If values of type `$t` can be transmuted into values of a primitive atomic type, declares
    // variable `$a` of that type and executes `$atomic_op`. Otherwise, just executes
    // `$fallback_op`.
    ($t:ty, $a:ident, $atomic_op:expr, $fallback_op:expr) => {
        loop {
            atomic!(@check, $t, AtomicUnit, $a, $atomic_op);
            atomic!(@check, $t, atomic::AtomicUsize, $a, $atomic_op);

            #[cfg(feature = "nightly")]
            {
                #[cfg(target_has_atomic = "8")]
                atomic!(@check, $t, atomic::AtomicU8, $a, $atomic_op);
                #[cfg(target_has_atomic = "16")]
                atomic!(@check, $t, atomic::AtomicU16, $a, $atomic_op);
                #[cfg(target_has_atomic = "32")]
                atomic!(@check, $t, atomic::AtomicU32, $a, $atomic_op);
                #[cfg(target_has_atomic = "64")]
                atomic!(@check, $t, atomic::AtomicU64, $a, $atomic_op);
            }

            break $fallback_op
        }
    };
}

/// Returns `true` if operations on `AtomicCell<T>` are lock-free.
fn atomic_is_lock_free<T>() -> bool {
    atomic! { T, _a, true, false }
}

/// Atomically reads data from `src`.
///
/// This operation uses the `SeqCst` ordering. If possible, an atomic instructions is used, and a
/// global lock otherwise.
unsafe fn atomic_load<T>(src: *mut T) -> T
where
    T: Copy,
{
    atomic! {
        T, a,
        {
            a = &*(src as *const _ as *const _);
            mem::transmute_copy(&a.load(Ordering::SeqCst))
        },
        {
            let _lock = lock(src as usize);
            ptr::read(src)
        }
    }
}

/// Atomically writes `val` to `dst`.
///
/// This operation uses the `SeqCst` ordering. If possible, an atomic instructions is used, and a
/// global lock otherwise.
unsafe fn atomic_store<T>(dst: *mut T, val: T) {
    atomic! {
        T, a,
        {
            a = &*(dst as *const _ as *const _);
            let res = a.store(mem::transmute_copy(&val), Ordering::SeqCst);
            mem::forget(val);
            res
        },
        {
            let _lock = lock(dst as usize);
            ptr::write(dst, val)
        }
    }
}

/// Atomically swaps data at `dst` with `val`.
///
/// This operation uses the `SeqCst` ordering. If possible, an atomic instructions is used, and a
/// global lock otherwise.
unsafe fn atomic_swap<T>(dst: *mut T, val: T) -> T {
    atomic! {
        T, a,
        {
            a = &*(dst as *const _ as *const _);
            let res = mem::transmute_copy(&a.swap(mem::transmute_copy(&val), Ordering::SeqCst));
            mem::forget(val);
            res
        },
        {
            let _lock = lock(dst as usize);
            ptr::replace(dst, val)
        }
    }
}

/// Atomically compares data at `dst` to `current` and, if equal byte-for-byte, exchanges data at
/// `dst` with `new`.
///
/// Returns the old value on success, or the current value at `dst` on failure.
///
/// This operation uses the `SeqCst` ordering. If possible, an atomic instructions is used, and a
/// global lock otherwise.
unsafe fn atomic_compare_exchange_weak<T>(dst: *mut T, current: T, new: T) -> Result<T, T>
where
    T: Copy,
{
    atomic! {
        T, a,
        {
            a = &*(dst as *const _ as *const _);
            let res = a.compare_exchange_weak(
                mem::transmute_copy(&current),
                mem::transmute_copy(&new),
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
            match res {
                Ok(v) => Ok(mem::transmute_copy(&v)),
                Err(v) => Err(mem::transmute_copy(&v)),
            }
        },
        {
            let _lock = lock(dst as usize);
            if byte_eq(&*dst, &current) {
                Ok(ptr::replace(dst, new))
            } else {
                Err(ptr::read(dst))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering::SeqCst;

    use super::AtomicCell;

    #[test]
    fn is_lock_free() {
        struct UsizeWrap(usize);
        struct U8Wrap(bool);

        assert_eq!(AtomicCell::<usize>::is_lock_free(), true);
        assert_eq!(AtomicCell::<isize>::is_lock_free(), true);
        assert_eq!(AtomicCell::<UsizeWrap>::is_lock_free(), true);

        assert_eq!(AtomicCell::<u8>::is_lock_free(), cfg!(feature = "nightly"));
        assert_eq!(AtomicCell::<bool>::is_lock_free(), cfg!(feature = "nightly"));
        assert_eq!(AtomicCell::<U8Wrap>::is_lock_free(), cfg!(feature = "nightly"));
    }

    #[test]
    fn drops_unit() {
        static CNT: AtomicUsize = AtomicUsize::new(0);
        CNT.store(0, SeqCst);

        #[derive(Debug, PartialEq, Eq)]
        struct Foo();

        impl Foo {
            fn new() -> Foo {
                CNT.fetch_add(1, SeqCst);
                Foo()
            }
        }

        impl Drop for Foo {
            fn drop(&mut self) {
                CNT.fetch_sub(1, SeqCst);
            }
        }

        impl Default for Foo {
            fn default() -> Foo {
                Foo::new()
            }
        }

        let a = AtomicCell::new(Foo::new());

        assert_eq!(a.swap(Foo::new()), Foo::new());
        assert_eq!(CNT.load(SeqCst), 1);

        a.store(Foo::new());
        assert_eq!(CNT.load(SeqCst), 1);

        assert_eq!(a.swap(Foo::default()), Foo::new());
        assert_eq!(CNT.load(SeqCst), 1);

        drop(a);
        assert_eq!(CNT.load(SeqCst), 0);
    }

    #[test]
    fn drops_u8() {
        static CNT: AtomicUsize = AtomicUsize::new(0);
        CNT.store(0, SeqCst);

        #[derive(Debug, PartialEq, Eq)]
        struct Foo(u8);

        impl Foo {
            fn new(val: u8) -> Foo {
                CNT.fetch_add(1, SeqCst);
                Foo(val)
            }
        }

        impl Drop for Foo {
            fn drop(&mut self) {
                CNT.fetch_sub(1, SeqCst);
            }
        }

        impl Default for Foo {
            fn default() -> Foo {
                Foo::new(0)
            }
        }

        let a = AtomicCell::new(Foo::new(5));

        assert_eq!(a.swap(Foo::new(6)), Foo::new(5));
        assert_eq!(a.swap(Foo::new(1)), Foo::new(6));
        assert_eq!(CNT.load(SeqCst), 1);

        a.store(Foo::new(2));
        assert_eq!(CNT.load(SeqCst), 1);

        assert_eq!(a.swap(Foo::default()), Foo::new(2));
        assert_eq!(CNT.load(SeqCst), 1);

        assert_eq!(a.swap(Foo::default()), Foo::new(0));
        assert_eq!(CNT.load(SeqCst), 1);

        drop(a);
        assert_eq!(CNT.load(SeqCst), 0);
    }

    #[test]
    fn drops_usize() {
        static CNT: AtomicUsize = AtomicUsize::new(0);
        CNT.store(0, SeqCst);

        #[derive(Debug, PartialEq, Eq)]
        struct Foo(usize);

        impl Foo {
            fn new(val: usize) -> Foo {
                CNT.fetch_add(1, SeqCst);
                Foo(val)
            }
        }

        impl Drop for Foo {
            fn drop(&mut self) {
                CNT.fetch_sub(1, SeqCst);
            }
        }

        impl Default for Foo {
            fn default() -> Foo {
                Foo::new(0)
            }
        }

        let a = AtomicCell::new(Foo::new(5));

        assert_eq!(a.swap(Foo::new(6)), Foo::new(5));
        assert_eq!(a.swap(Foo::new(1)), Foo::new(6));
        assert_eq!(CNT.load(SeqCst), 1);

        a.store(Foo::new(2));
        assert_eq!(CNT.load(SeqCst), 1);

        assert_eq!(a.swap(Foo::default()), Foo::new(2));
        assert_eq!(CNT.load(SeqCst), 1);

        assert_eq!(a.swap(Foo::default()), Foo::new(0));
        assert_eq!(CNT.load(SeqCst), 1);

        drop(a);
        assert_eq!(CNT.load(SeqCst), 0);
    }

    #[test]
    fn modular_u8() {
        #[derive(Clone, Copy, Eq, Debug, Default)]
        struct Foo(u8);

        impl PartialEq for Foo {
            fn eq(&self, other: &Foo) -> bool {
                self.0 % 5 == other.0 % 5
            }
        }

        let a = AtomicCell::new(Foo(1));

        assert_eq!(a.load(), Foo(1));
        assert_eq!(a.swap(Foo(2)), Foo(11));
        assert_eq!(a.load(), Foo(52));

        a.store(Foo(0));
        assert_eq!(a.compare_exchange(Foo(0), Foo(5)), Ok(Foo(100)));
        assert_eq!(a.load().0, 5);
        assert_eq!(a.compare_exchange(Foo(10), Foo(15)), Ok(Foo(100)));
        assert_eq!(a.load().0, 15);
    }

    #[test]
    fn modular_usize() {
        #[derive(Clone, Copy, Eq, Debug, Default)]
        struct Foo(usize);

        impl PartialEq for Foo {
            fn eq(&self, other: &Foo) -> bool {
                self.0 % 5 == other.0 % 5
            }
        }

        let a = AtomicCell::new(Foo(1));

        assert_eq!(a.load(), Foo(1));
        assert_eq!(a.swap(Foo(2)), Foo(11));
        assert_eq!(a.load(), Foo(52));

        a.store(Foo(0));
        assert_eq!(a.compare_exchange(Foo(0), Foo(5)), Ok(Foo(100)));
        assert_eq!(a.load().0, 5);
        assert_eq!(a.compare_exchange(Foo(10), Foo(15)), Ok(Foo(100)));
        assert_eq!(a.load().0, 15);
    }
}
