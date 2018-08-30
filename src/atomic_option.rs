use std::marker::PhantomData;
use std::mem::transmute;
use std::sync::{
    atomic::{AtomicPtr, Ordering},
    Arc,
};

/// trait for reference-like objects where `Option<Self>` can be safely transmuted
/// to a `*mut Self::Target`, sent to another thread (unless `Self` is not `Send`),
/// and transmuted back
pub unsafe trait ReferenceLike: Sized {
    /// the type that `Self` is a reference to; usually `Deref::Target`
    type Target;
    /// transmute `Option<Self>` into a `*mut Self::Target` without panicking
    unsafe fn to_ptr(value: Option<Self>) -> *mut Self::Target;
    /// transmute a `*mut Self::Target` back into a `Option<Self>` without panicking
    unsafe fn from_ptr(value: *mut Self::Target) -> Option<Self>;
}

macro_rules! impl_reference_like {
    () => {
        type Target = T;
        unsafe fn to_ptr(value: Option<Self>) -> *mut Self::Target {
            transmute(value)
        }
        unsafe fn from_ptr(value: *mut Self::Target) -> Option<Self> {
            transmute(value)
        }
    };
}

unsafe impl<T> ReferenceLike for Box<T> {
    impl_reference_like!();
}

unsafe impl<T> ReferenceLike for Arc<T> {
    impl_reference_like!();
}

unsafe impl<'a, T> ReferenceLike for &'a T {
    impl_reference_like!();
}

unsafe impl<'a, T> ReferenceLike for &'a mut T {
    impl_reference_like!();
}

/// an atomic `Option` that only works with reference-like values
#[repr(transparent)]
#[derive(Debug)]
pub struct AtomicOption<R: ReferenceLike> {
    value: AtomicPtr<R::Target>,
    _phantom: PhantomData<R>,
}

impl<R: ReferenceLike> AtomicOption<R> {
    /// create a new `AtomicOption`
    pub fn new(value: Option<R>) -> Self {
        Self {
            value: AtomicPtr::new(unsafe { ReferenceLike::to_ptr(value) }),
            _phantom: PhantomData,
        }
    }
    /// atomically replace the value in `self` with `new_value`, returning the old value
    pub fn replace(&self, new_value: Option<R>) -> Option<R> {
        unsafe {
            ReferenceLike::from_ptr(
                self.value
                    .swap(ReferenceLike::to_ptr(new_value), Ordering::AcqRel),
            )
        }
    }
    /// atomically replace the value in `self` with `None`, returning the old value
    pub fn take(&self) -> Option<R> {
        self.replace(None)
    }
    /// atomically sets the value in `self` to `new_value`, dropping the old value
    pub fn set(&self, new_value: Option<R>) {
        self.replace(new_value);
    }
    /// consumes `self` and returns the contained value
    pub fn into_inner(mut self) -> Option<R> {
        unsafe { ReferenceLike::from_ptr(*self.value.get_mut()) }
    }
}

impl<R: ReferenceLike> Default for AtomicOption<R> {
    fn default() -> Self {
        Self::new(None)
    }
}

unsafe impl<R: ReferenceLike + Send> Send for AtomicOption<R> {}

unsafe impl<R: ReferenceLike + Send> Sync for AtomicOption<R> {}

impl<R: ReferenceLike> Drop for AtomicOption<R> {
    fn drop(&mut self) {
        self.set(None)
    }
}

impl<R: ReferenceLike> From<Option<R>> for AtomicOption<R> {
    fn from(value: Option<R>) -> Self {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, ATOMIC_BOOL_INIT};

    #[test]
    fn simple() {
        let atomic_option = AtomicOption::new(Some(Box::new(0u8)));
        assert_eq!(atomic_option.take(), Some(Box::new(0)));
        assert_eq!(atomic_option.take(), None);
        assert_eq!(atomic_option.replace(Some(Box::new(1))), None);
        assert_eq!(atomic_option.replace(Some(Box::new(2))), Some(Box::new(1)));
    }

    #[test]
    fn test_drop() {
        static DID_DROP: AtomicBool = ATOMIC_BOOL_INIT;

        struct Dropable;

        impl Drop for Dropable {
            fn drop(&mut self) {
                DID_DROP.store(true, Ordering::Relaxed);
            }
        }

        let atomic_option: AtomicOption<Box<_>> = Some(Dropable.into()).into();
        atomic_option.set(None);
        assert!(DID_DROP.swap(false, Ordering::Relaxed));
        atomic_option.set(Some(Dropable.into()).into());
        {
            atomic_option
        };
        assert!(DID_DROP.load(Ordering::Relaxed));
    }
}
