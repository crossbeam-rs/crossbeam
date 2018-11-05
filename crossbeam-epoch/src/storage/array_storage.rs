use alloc::vec::Vec;
use core::marker::PhantomData;
use core::mem;
use core::mem::ManuallyDrop;
use core::ops::{Deref, DerefMut};
use core::ptr;

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
pub struct Array<T> {
    anchor: ManuallyDrop<T>,
}

impl<T> Array<T> {
    /// Returns its size.
    pub fn size(&self) -> usize {
        let usize_align = mem::align_of::<usize>();
        let usize_size = mem::size_of::<usize>();
        let t_align = mem::align_of::<T>();
        let t_size = mem::size_of::<T>();

        unsafe {
            // The memory layout varies depending on whether `T`'s alignment is <= `usize`'s
            // alignment or not.
            if t_align <= usize_align {
                // Size is located a word before the anchor.
                let ptr_num = (&self.anchor as *const _ as *const usize).sub(1);
                ptr::read(ptr_num)
            } else {
                // Size is located `usize_elts * sizeof(T)`-bytes before the anchor.
                let usize_elts = div_ceil(usize_size, t_size);
                let ptr_num =
                    (&self.anchor as *const ManuallyDrop<T>).sub(usize_elts) as *const usize;
                ptr::read(ptr_num)
            }
        }
    }

    /// Returns the pointer to `index`-th element.
    ///
    /// # Safety
    ///
    /// `index` should be less than its size.  Otherwise, the behavior is undefined.
    pub unsafe fn at(&self, index: usize) -> *const ManuallyDrop<T> {
        debug_assert!(
            index < self.size(),
            "Array::at(): index {} should be < size {}",
            index,
            self.size()
        );

        let anchor = &self.anchor as *const ManuallyDrop<T>;
        anchor.add(index)
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

fn div_ceil(a: usize, b: usize) -> usize {
    (a + b - 1) / b
}

impl<T> ArrayBox<T> {
    /// Creates a new array and returns the owning pointer to the new array.
    pub fn new(num: usize) -> Self {
        let usize_align = mem::align_of::<usize>();
        let usize_size = mem::size_of::<usize>();
        let t_align = mem::align_of::<T>();
        let t_size = mem::size_of::<T>();

        // The memory layout varies depending on whether `T`'s alignment is <= `usize`'s alignment
        // or not.
        if t_align <= usize_align {
            let t_bytes = num * t_size;
            let t_words = div_ceil(t_bytes, usize_size);

            let mut vec = Vec::<usize>::with_capacity(1 + t_words);
            let ptr = vec.as_mut_ptr();
            mem::forget(vec);

            unsafe {
                ptr::write(ptr, num);
            }

            Self {
                ptr: unsafe { (ptr.add(1)) } as *mut Array<T>,
                _marker: PhantomData,
            }
        } else {
            let usize_elts = div_ceil(usize_size, t_size);

            let mut vec = Vec::<T>::with_capacity(usize_elts + num);
            let ptr = vec.as_mut_ptr();
            mem::forget(vec);

            unsafe {
                ptr::write(ptr as *mut usize, num);
            }

            Self {
                ptr: unsafe { (ptr.add(usize_elts)) } as *mut Array<T>,
                _marker: PhantomData,
            }
        }
    }
}

impl<T> Drop for ArrayBox<T> {
    /// Destroys the array it owns.
    fn drop(&mut self) {
        let usize_align = mem::align_of::<usize>();
        let usize_size = mem::size_of::<usize>();
        let t_align = mem::align_of::<T>();
        let t_size = mem::size_of::<T>();

        unsafe {
            // The memory layout varies depending on whether `T`'s alignment is <= `usize`'s
            // alignment or not.
            if t_align <= usize_align {
                let ptr_num = (self.ptr as *mut usize).sub(1);
                let num = ptr::read(ptr_num);

                let t_bytes = num * t_size;
                let t_words = div_ceil(t_bytes, usize_size);

                drop(Vec::from_raw_parts(ptr_num, 0, 1 + t_words));
            } else {
                let usize_elts = div_ceil(usize_size, t_size);
                let ptr_num = self.ptr.sub(usize_elts) as *mut usize;
                let num = ptr::read(ptr_num);

                drop(Vec::from_raw_parts(ptr_num as *mut T, 0, usize_elts + num));
            }
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
