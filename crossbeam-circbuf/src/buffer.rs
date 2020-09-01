use core::cell::UnsafeCell;
use core::mem::{ManuallyDrop, MaybeUninit};
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicUsize, Ordering};

use epoch::{Owned, Pointable};

/// A slot in buffer.
#[derive(Debug)]
pub struct Slot<T> {
    /// The index of this slot's data.
    ///
    /// An index consists of an offset into the buffer and a lap, but packed into a single `usize`.
    /// The lower bits represent the offset, while the upper bits represent the lap.  For example if
    /// the buffer capacity is 4, then index 6 represents offset 2 in lap 1.
    index: AtomicUsize,

    /// The value in this slot.
    ///
    /// The allocated memory exhibit data races by `read`, `write`, `read_index`, and `write_index`,
    /// which technically invoke undefined behavior.  We should be using relaxed accesses, but that
    /// would cost too much performance.  Hence, as a HACK, we use volatile accesses instead.
    /// Experimental evidence shows that this works.
    data: UnsafeCell<ManuallyDrop<T>>,
}

/// A buffer that holds values in a queue.
///
/// This is just a buffer---dropping an instance of this struct will *not* drop the internal values.
pub struct Buffer<T> {
    inner: [MaybeUninit<Slot<T>>],
}

impl<T> Pointable for Buffer<T> {
    const ALIGN: usize = <[MaybeUninit<Slot<T>>] as Pointable>::ALIGN;

    type Init = <[MaybeUninit<Slot<T>>] as Pointable>::Init;

    unsafe fn init(size: Self::Init) -> usize {
        <[MaybeUninit<Slot<T>>] as Pointable>::init(size)
    }

    unsafe fn deref<'a>(ptr: usize) -> &'a Self {
        &*(<[MaybeUninit<Slot<T>>] as Pointable>::deref(ptr) as *const _ as *const _)
    }

    unsafe fn deref_mut<'a>(ptr: usize) -> &'a mut Self {
        &mut *(<[MaybeUninit<Slot<T>>] as Pointable>::deref_mut(ptr) as *mut _ as *mut _)
    }

    unsafe fn drop(ptr: usize) {
        <[MaybeUninit<Slot<T>>] as Pointable>::drop(ptr)
    }
}

impl<T> Deref for Buffer<T> {
    type Target = [MaybeUninit<Slot<T>>];

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> DerefMut for Buffer<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<T> Buffer<T> {
    /// Allocates a new buffer with the specified capacity.
    pub fn new(size: usize) -> Owned<Self> {
        // `size` should be a power of two.
        debug_assert_eq!(size, size.next_power_of_two());

        let mut buffer = Owned::<Self>::init(size);

        // Marks all entries invalid.
        for (i, slot) in buffer.deref_mut().iter_mut().enumerate() {
            // Index `i + 1` for the `i`-th entry is invalid; only the indexes of the form `i + N *
            // size` is valid.
            unsafe {
                (*slot.as_ptr()).index.store(i + 1, Ordering::Relaxed);
            }
        }

        buffer
    }

    /// Returns the capacity of the buffer.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns a pointer to the slot at the specified `index`.
    pub fn get(&self, index: usize) -> &Slot<T> {
        // `array.size()` is always a power of two.
        unsafe { &*self.inner.get_unchecked(index & (self.len() - 1)).as_ptr() }
    }

    /// Reads a value from the specified `index`.
    ///
    /// Returns `Some(v)` if `v` is at `index`; or `None` if there's no valid value for `index`.
    pub unsafe fn read(&self, index: usize) -> Option<ManuallyDrop<T>> {
        let slot = self.get(index);

        // Reads the index with `Acquire`.
        let i = slot.index.load(Ordering::Acquire);

        // If the index in the buffer mismatches with the queried index, there's no valid value.
        if index != i {
            return None;
        }

        // Returns the value.
        Some(slot.data.get().read_volatile())
    }

    /// Reads a value from the specified `index` without checking the index.
    ///
    /// Returns the value at `index` regardless or whether it's valid or not.
    pub unsafe fn read_unchecked(&self, index: usize) -> ManuallyDrop<T> {
        let slot = self.get(index);
        slot.data.get().read_volatile()
    }

    /// Reads the index from the specified slot.
    pub fn read_index(&self, index: usize, ord: Ordering) -> usize {
        let slot = self.get(index);
        slot.index.load(ord)
    }

    /// Writes `value` into the specified `index`.
    pub unsafe fn write(&self, index: usize, value: T) {
        let slot = self.get(index);

        // Writes the value.
        slot.data.get().write_volatile(ManuallyDrop::new(value));

        // Writes the index with `Release`.
        slot.index.store(index, Ordering::Release);
    }

    /// Writes the specified `index` in the slot.
    pub fn write_index(&self, index: usize, ord: Ordering) {
        let slot = self.get(index);
        slot.index.store(index, ord);
    }
}
