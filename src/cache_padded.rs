use core::fmt;
use core::mem;
use core::ops::{Deref, DerefMut};
use core::ptr;


cfg_if! {
    if #[cfg(feature = "nightly")] {
        // This trick allows use to support rustc 1.12.1, which does not support the
        // #[repr(align(n))] syntax. Using the attribute makes the parser fail over.
        // It is, however, okay to use it within a macro, since it would be parsed
        // in a later stage, but that never occurs due to the cfg_if.
        // TODO(Vtec234): remove this crap when we drop support for 1.12.
        macro_rules! nightly_inner {
            () => (
                #[derive(Clone)]
                #[repr(align(64))]
                pub(crate) struct Inner<T> {
                    value: T,
                }
            )
        }
        nightly_inner!();

        impl<T> Inner<T> {
            pub(crate) fn new(t: T) -> Inner<T> {
                Self {
                    value: t
                }
            }
        }

        impl<T> Deref for Inner<T> {
            type Target = T;

            fn deref(&self) -> &T {
                &self.value
            }
        }

        impl<T> DerefMut for Inner<T> {
            fn deref_mut(&mut self) -> &mut T {
                &mut self.value
            }
        }
    } else {
        use core::marker::PhantomData;

        struct Inner<T> {
            bytes: [u8; 64],

            /// `[T; 0]` ensures alignment is at least that of `T`.
            /// `PhantomData<T>` signals that `CachePadded<T>` contains a `T`.
            _marker: ([T; 0], PhantomData<T>),
        }

        impl<T> Inner<T> {
            fn new(t: T) -> Inner<T> {
                assert!(mem::size_of::<T>() <= mem::size_of::<Self>());
                assert!(mem::align_of::<T>() <= mem::align_of::<Self>());

                unsafe {
                    let mut inner: Self = mem::uninitialized();
                    let p: *mut T = &mut *inner;
                    ptr::write(p, t);
                    inner
                }
            }
        }

        impl<T> Deref for Inner<T> {
            type Target = T;

            fn deref(&self) -> &T {
                unsafe { &*(self.bytes.as_ptr() as *const T) }
            }
        }

        impl<T> DerefMut for Inner<T> {
            fn deref_mut(&mut self) -> &mut T {
                unsafe { &mut *(self.bytes.as_ptr() as *mut T) }
            }
        }

        impl<T> Drop for CachePadded<T> {
            fn drop(&mut self) {
                let p: *mut T = self.deref_mut();
                unsafe {
                    ptr::drop_in_place(p);
                }
            }
        }

        impl<T: Clone> Clone for Inner<T> {
            fn clone(&self) -> Inner<T> {
                let val = self.deref().clone();
                Self::new(val)
            }
        }
    }
}

/// Pads `T` to the length of a cache line.
///
/// Sometimes concurrent programming requires a piece of data to be padded out to the size of a
/// cacheline to avoid "false sharing": cache lines being invalidated due to unrelated concurrent
/// activity. Use this type when you want to *avoid* cache locality.
///
/// At the moment, cache lines are assumed to be 64 bytes on all architectures.
///
/// # Size and alignment
///
/// By default, the size of `CachePadded<T>` is 64 bytes. If `T` is larger than that, then
/// `CachePadded::<T>::new` will panic. Alignment of `CachePadded<T>` is the same as that of `T`.
///
/// However, if the `nightly` feature is enabled, arbitrarily large types `T` can be stored inside
/// a `CachePadded<T>`. The size will then be a multiple of 64 at least the size of `T`, and the
/// alignment will be the maximum of 64 and the alignment of `T`.
pub struct CachePadded<T> {
    inner: Inner<T>,
}

unsafe impl<T: Send> Send for CachePadded<T> {}
unsafe impl<T: Sync> Sync for CachePadded<T> {}

impl<T> CachePadded<T> {
    /// Pads a value to the length of a cache line.
    ///
    /// # Panics
    ///
    /// If `nightly` is not enabled and `T` is larger than 64 bytes, this function will panic.
    pub fn new(t: T) -> CachePadded<T> {
        CachePadded::<T> { inner: Inner::new(t) }
    }
}

impl<T> Deref for CachePadded<T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.inner.deref()
    }
}

impl<T> DerefMut for CachePadded<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.inner.deref_mut()
    }
}

impl<T: Default> Default for CachePadded<T> {
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl<T: Clone> Clone for CachePadded<T> {
    fn clone(&self) -> Self {
        CachePadded { inner: self.inner.clone() }
    }
}

impl<T: fmt::Debug> fmt::Debug for CachePadded<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let inner: &T = &*self;
        write!(f, "CachePadded {{ {:?} }}", inner)
    }
}

impl<T> From<T> for CachePadded<T> {
    fn from(t: T) -> Self {
        CachePadded::new(t)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn store_u64() {
        let x: CachePadded<u64> = CachePadded::new(17);
        assert_eq!(*x, 17);
    }

    #[test]
    fn store_pair() {
        let x: CachePadded<(u64, u64)> = CachePadded::new((17, 37));
        assert_eq!(x.0, 17);
        assert_eq!(x.1, 37);
    }

    #[test]
    fn distance() {
        let arr = [CachePadded::new(17u8), CachePadded::new(37u8)];
        let a = &*arr[0] as *const u8;
        let b = &*arr[1] as *const u8;
        assert!(unsafe { a.offset(64) } <= b);
    }

    #[test]
    fn different_sizes() {
        CachePadded::new(17u8);
        CachePadded::new(17u16);
        CachePadded::new(17u32);
        CachePadded::new([17u64; 0]);
        CachePadded::new([17u64; 1]);
        CachePadded::new([17u64; 2]);
        CachePadded::new([17u64; 3]);
        CachePadded::new([17u64; 4]);
        CachePadded::new([17u64; 5]);
        CachePadded::new([17u64; 6]);
        CachePadded::new([17u64; 7]);
        CachePadded::new([17u64; 8]);
    }

    cfg_if! {
        if #[cfg(feature = "nightly")] {
            #[test]
            fn large() {
                let a = [17u64; 9];
                let b = CachePadded::new(a);
                assert!(mem::size_of_val(&a) <= mem::size_of_val(&b));
            }
        } else {
            #[test]
            #[should_panic]
            fn large() {
                CachePadded::new([17u64; 9]);
            }
        }
    }

    #[test]
    fn debug() {
        assert_eq!(
            format!("{:?}", CachePadded::new(17u64)),
            "CachePadded { 17 }"
        );
    }

    #[test]
    fn drops() {
        let count = Cell::new(0);

        struct Foo<'a>(&'a Cell<usize>);

        impl<'a> Drop for Foo<'a> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        let a = CachePadded::new(Foo(&count));
        let b = CachePadded::new(Foo(&count));

        assert_eq!(count.get(), 0);
        drop(a);
        assert_eq!(count.get(), 1);
        drop(b);
        assert_eq!(count.get(), 2);
    }

    #[test]
    fn clone() {
        let a = CachePadded::new(17);
        let b = a.clone();
        assert_eq!(*a, *b);
    }

    #[test]
    fn runs_custom_clone() {
        let count = Cell::new(0);

        struct Foo<'a>(&'a Cell<usize>);

        impl<'a> Clone for Foo<'a> {
            fn clone(&self) -> Foo<'a> {
                self.0.set(self.0.get() + 1);
                Foo::<'a>(self.0)
            }
        }

        let a = CachePadded::new(Foo(&count));
        a.clone();

        assert_eq!(count.get(), 1);
    }
}
