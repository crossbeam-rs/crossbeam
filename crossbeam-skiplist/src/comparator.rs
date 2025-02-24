//! Traits for key comparison in maps.

use core::{borrow::Borrow, cmp::Ordering};

/// Key equality trait.
///
/// This trait allows for very flexible comparison of objects. You may
/// borrow/dereference `L` and `R` using `Borrow` or another trait. The trait
/// takes a `self` parameter, which you can use to change how the operands are
/// compared. For example, you can toggle string case-sensitivity on and off at
/// runtime, or you can use `Box<dyn Equivalator<L, R>>` to allow the user of
/// your code to supply a custom comparison function.
///
/// Because you can use `Equivalator` to implement very different comparison
/// logic from the `Eq` trait for a given type, there is no expectation that
/// two keys should have the same hash for them to compare equal. For example,
/// this is a valid implementation that treats two inputs of any type as equal:
/// ```
/// use crossbeam_skiplist::comparator::Equivalator;
///
/// struct TrivialEquivalator;
///
/// impl<L: ?Sized, R: ?Sized> Equivalator<L, R> for TrivialEquivalator {
///     fn equivalent(&self, lhs: &L, rhs: &R) -> bool {
///         true
///     }
/// }
/// ```
///
/// See also `Comparator`.
pub trait Equivalator<L: ?Sized, R: ?Sized = L> {
    /// Compare `lhs` to `rhs` for equality.
    fn equivalent(&self, lhs: &L, rhs: &R) -> bool;
}

/// Key ordering trait.
///
/// This trait allows for very flexible comparison of objects. You may
/// borrow/dereference `L` and `R` using `Borrow` or another trait. The trait
/// takes a `self` parameter, which you can use to change how the operands are
/// compared. For example, you can toggle string case-sensitivity on and off,
/// or you can use `Box<dyn Comparator<L, R>>` to allow the user of your code
/// to supply a custom comparison function.
///
/// The `Comparator` and `Equivalator` implementations for a comparator must
/// agree on which inputs are equal.
///
/// See also `Equivalator`.
pub trait Comparator<L: ?Sized, R: ?Sized = L>: Equivalator<L, R> {
    /// Compare `lhs` to `rhs` and return their ordering.
    fn compare(&self, lhs: &L, rhs: &R) -> Ordering;
}

/// This comparator uses the standard library `Borrow` and `Ord` traits to
/// compare values. When used in a data structure, this results in the same
/// behavior as the standard `BTreeMap` interface.
#[derive(Clone, Copy, Debug, Default)]
pub struct OrdComparator;

impl<K: ?Sized, Q: ?Sized> Equivalator<K, Q> for OrdComparator
where
    K: Borrow<Q>,
    Q: Eq,
{
    #[inline]
    fn equivalent(&self, lhs: &K, rhs: &Q) -> bool {
        <Q as PartialEq>::eq(lhs.borrow(), rhs)
    }
}

impl<K: ?Sized, Q: ?Sized> Comparator<K, Q> for OrdComparator
where
    K: Borrow<Q>,
    Q: Ord,
{
    #[inline]
    fn compare(&self, lhs: &K, rhs: &Q) -> Ordering {
        Ord::cmp(lhs.borrow(), rhs)
    }
}
