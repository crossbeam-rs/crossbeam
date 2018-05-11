use core::fmt;
use core::ops::{Deref, DerefMut};

/// Pads `T` to the length of a cache line.
///
/// Sometimes concurrent programming requires a piece of data to be padded out to the size of a
/// cacheline to avoid "false sharing": cache lines being invalidated due to unrelated concurrent
/// activity. Use this type when you want to *avoid* cache locality.
///
/// Cache lines are assumed to be 64 bytes on all architectures.
///
/// # Size and alignment
///
/// The size of `CachePadded<T>` is the smallest multiple of 64 bytes large enough to accommodate
/// a value of type `T`.
///
/// The alignment of `CachePadded<T>` is the maximum of 64 bytes and the alignment of `T`.
#[derive(Clone, Default)]
#[repr(align(64))]
pub struct CachePadded<T> {
    inner: T,
}

unsafe impl<T: Send> Send for CachePadded<T> {}
unsafe impl<T: Sync> Sync for CachePadded<T> {}

impl<T> CachePadded<T> {
    /// Pads a value to the length of a cache line.
    pub fn new(t: T) -> CachePadded<T> {
        CachePadded::<T> { inner: t }
    }
}

impl<T> Deref for CachePadded<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T> DerefMut for CachePadded<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
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
    use core::mem;
    use core::cell::Cell;

    #[test]
    fn default() {
        let x: CachePadded<u64> = Default::default();
        assert_eq!(*x, 0);
    }

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

    #[test]
    fn large() {
        let a = [17u64; 9];
        let b = CachePadded::new(a);
        assert!(mem::size_of_val(&a) <= mem::size_of_val(&b));
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
