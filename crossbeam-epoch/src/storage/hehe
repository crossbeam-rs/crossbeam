use alloc::alloc;
use core::marker::PhantomData;
use core::mem;
use core::mem::ManuallyDrop;
use core::ops::{Deref, DerefMut};

use {AtomicTmpl, OwnedTmpl, SharedTmpl, Storage};

/// An atomic pointer to an array that can be safely shared between threads.
///
/// See [`AtomicTmpl`] for more details.
///
/// [`AtomicTmpl`]: struct.AtomicTmpl.html
pub type AtomicArray<T> = AtomicTmpl<Array<T>, ArrayBox<T>>;

/// An owned heap-allocated array.
///
/// See [`OwnedTmpl`] for more details.
///
/// [`OwnedTmpl`]: struct.OwnedTmpl.html
pub type OwnedArray<T> = OwnedTmpl<Array<T>, ArrayBox<T>>;

/// A pointer to an array protected by the epoch GC.
///
/// See [`SharedTmpl`] for more details.
///
/// [`SharedTmpl`]: struct.SharedTmpl.html
pub type SharedArray<'g, T> = SharedTmpl<'g, Array<T>, ArrayBox<T>>;

/// An array consisting of its size and elements.
///
/// # Memory layout
///
/// The memory layout of an `Array<T>` is not ordinary: it actually owns memory locations outside of
/// its usual range of size `mem::size_of::<Array<T>>()`.  In particular, the ordinary range is just
/// the first element of the array, which we call the "anchor".  Size is stored before the anchor,
/// and the other elements are located after the anchor:
///
/// ```ignore
///          anchor
///          |
///          |
/// ------------------------------------
/// | size | 0 | 1 | 2 | 3 | 4 | 5 | 6 |
/// ------------------------------------
///
///        <---> (Array<T>'s ordinary range)
/// ```
///
/// The location of the size varies depending on whether `T`'s alignment is <= `usize`'s alignment
/// or not.  If so, the array is allocated as if it's an array of `usize`, and the size is stored a
/// word before the anchor (the beginning of the array); otherwise, the array is allocated as if
/// it's an array of `T`, and the size is stored `N * sizeof(T)` bytes before the anchor for an
/// appropriate `N`.
///
///
/// # Ownership
///
/// `Array` is just a storage and doesn't own any elements inside the array.  In particular, it
/// doesn't call `drop()` for its elements.
///
/// # Lifetime management
///
/// Because of non-standard memory layout, `Array` doesn't provide an ordinary constructor or
/// destroyer.  Instead, the lifetime of an array is always managed by its owning pointer of type
/// [`ArrayBox`].  See [`ArrayBox::new`] and [`ArrayBox::drop`] for more details.
///
/// [`ArrayBox`]: struct.ArrayBox.html
/// [`ArrayBox::new`]: struct.ArrayBox.html#method.new
/// [`ArrayBox::drop`]: struct.ArrayBox.html#impl-Drop
#[derive(Debug)]
#[repr(C)]
pub struct Array<T> {
    size: usize,
    anchor: [ManuallyDrop<T>; 0],
}

impl<T> Array<T> {
    /// Returns its size.
    #[inline]
    pub fn size(&self) -> usize {
        self.size
    }

    /// Returns the pointer to `index`-th element.
    ///
    /// # Safety
    ///
    /// `index` should be less than its size.  Otherwise, the behavior is undefined.
    pub unsafe fn at(&self, index: usize) -> *const ManuallyDrop<T> {
        debug_assert!(
            index < self.size,
            "Array::at(): index {} should be < size {}",
            index,
            self.size
        );

        &self.anchor[index] as *const _
    }
}

/// The storage type for an [`Array`].
///
///
/// See [`Array`] for more details.
///
/// # Examples
///
/// ```
/// use crossbeam_epoch::{self as epoch, OwnedArray, Array, ArrayBox};
///
/// let a = ArrayBox::<i32>::new(10);
/// let o = OwnedArray::from(a);
/// ```
///
/// [`Array`]: struct.Array.html
#[derive(Debug)]
pub struct ArrayBox<T> {
    ptr: *mut Array<T>,
    _marker: PhantomData<T>,
}

impl<T> ArrayBox<T> {
    /// Creates a new array and returns the owning pointer to the new array.
    pub fn new(size: usize) -> Self {
        let size = mem::size_of::<Array<T>>() + mem::size_of::<ManuallyDrop<T>>() * size;
        let align = mem::align_of::<Array<T>>();
        let layout = alloc::Layout::from_size_align(size, align).unwrap();
        let ptr = unsafe { alloc::alloc(layout) } as *const Array<T> as *mut Array<T>;
        (unsafe { &mut *ptr }).size = size;
        Self {
            ptr,
            _marker: PhantomData,
        }
    }
}

impl<T> Drop for ArrayBox<T> {
    /// Destroys the array it owns.
    fn drop(&mut self) {
        let size = self.size();
        let align = mem::align_of::<Array<T>>();
        let layout = alloc::Layout::from_size_align(size, align).unwrap();
        unsafe {
            alloc::dealloc(self.ptr as *mut u8, layout);
        }
    }
}

unsafe impl<T> Storage<Array<T>> for ArrayBox<T> {
    fn into_raw(self) -> *mut Array<T> {
        let ptr = self.ptr;
        mem::forget(self);
        ptr
    }

    unsafe fn from_raw(ptr: *mut Array<T>) -> Self {
        Self {
            ptr,
            _marker: PhantomData,
        }
    }
}

impl<T> Deref for ArrayBox<T> {
    type Target = Array<T>;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.ptr }
    }
}

impl<T> DerefMut for ArrayBox<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.ptr }
    }
}
