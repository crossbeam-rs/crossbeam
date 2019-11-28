use alloc::alloc;
use core::ptr;
use core::marker::PhantomData;
use core::mem;
use core::mem::ManuallyDrop;
use core::ops::{Deref, DerefMut};
use core::slice;

use atomic::{self, Handle};

/// An atomic pointer to an array that can be safely shared between threads.
///
/// See [`Atomic`] for more details.
///
/// [`Atomic`]: atomic/struct.Atomic.html
pub type ArrayAtomic<T> = atomic::Atomic<ArrayBox<T>>;

/// An owned heap-allocated array.
///
/// See [`Owned`] for more details.
///
/// [`Owned`]: atomic/struct.Owned.html
pub type ArrayOwned<T> = atomic::Owned<ArrayBox<T>>;

/// A pointer to an array protected by the epoch GC.
///
/// See [`Shared`] for more details.
///
/// [`Shared`]: atomic/struct.Shared.html
pub type ArrayShared<'g, T> = atomic::Shared<'g, ArrayBox<T>>;

#[derive(Debug)]
#[repr(C)]
struct Array<T> {
    size: usize,
    elements: [ManuallyDrop<T>; 0],
}

/// A box that owns an array.
///
/// # Memory layout
///
/// An array box points to the memory location containing array size and elements:
///
/// ```ignore
///          elements
///          |
///          |
/// ------------------------------------
/// | size | 0 | 1 | 2 | 3 | 4 | 5 | 6 |
/// ------------------------------------
/// ```
///
/// It is different from `Box<[T]>` in that `size` is in the allocation of array box, while `size`
/// is in `Box<[T]>`'s representation along with pointer.
///
/// # Ownership
///
/// An array box owns the array elements.  In particular, when an array box is dropped, its elements
/// are dropped.
///
/// # Examples
///
/// ```
/// use crossbeam_epoch::{self as epoch, ArrayOwned, ArrayBox};
///
/// let a = ArrayBox::<i32>::new(10);
/// let o = ArrayOwned::from(a);
/// ```
#[derive(Debug)]
pub struct ArrayBox<T> {
    ptr: *mut Array<T>,
    _marker: PhantomData<T>,
}

impl<T> ArrayBox<T> {
    /// Allocates a new array and returns an array box that owns the new array.
    ///
    /// # Safety
    ///
    /// The array is not initialized.
    #[inline]
    pub unsafe fn new_uninit(size: usize) -> Self
    {
        let size = mem::size_of::<Array<T>>() + mem::size_of::<ManuallyDrop<T>>() * size;
        let align = mem::align_of::<Array<T>>();
        let layout = alloc::Layout::from_size_align(size, align).unwrap();
        let ptr = alloc::alloc(layout) as *const Array<T> as *mut Array<T>;
        (*ptr).size = size;

        Self {
            ptr,
            _marker: PhantomData,
        }
    }

    /// Allocates a new array, initializes it with the given `value`, and returns an array box that
    /// owns the new array.
    pub fn new(size: usize, value: T) -> Self
    where
        T: Copy,
    {
        unsafe {
            let mut result = Self::new_uninit(size);
            for element in result.deref_mut() {
                ptr::write(element, value);
            }
            result
        }
    }
}

impl<T> Drop for ArrayBox<T> {
    fn drop(&mut self) {
        unsafe {
            for element in self.deref_mut() {
                ptr::drop_in_place(element);
            }

            let size = mem::size_of::<Array<T>>() + mem::size_of::<ManuallyDrop<T>>() * (*self.ptr).size;
            let align = mem::align_of::<Array<T>>();
            let layout = alloc::Layout::from_size_align(size, align).unwrap();
            alloc::dealloc(self.ptr as *mut u8, layout);
        }
    }
}

impl<T> Deref for ArrayBox<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        unsafe {
            let size = (*self.ptr).size;
            let elements = (*self.ptr).elements.as_ptr() as *const _;
            slice::from_raw_parts(elements, size)
        }
    }
}

impl<T> DerefMut for ArrayBox<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe {
            let size = (*self.ptr).size;
            let elements = (*self.ptr).elements.as_ptr() as *mut _;
            slice::from_raw_parts_mut(elements, size)
        }
    }
}

unsafe impl<T> Handle for ArrayBox<T> {
    const ALIGN: usize = mem::align_of::<T>();

    fn into_usize(self) -> usize {
        let ptr = self.ptr as usize;
        mem::forget(self);
        ptr
    }

    unsafe fn from_usize(ptr: usize) -> Self {
        Self {
            ptr: ptr as *mut _,
            _marker: PhantomData,
        }
    }
}
