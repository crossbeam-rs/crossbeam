use std::borrow::Borrow;
use std::fmt;
use std::iter::FromIterator;

use base::{self, try_pin_loop};
use epoch;
use Bound;

/// A map based on a lock-free skip list.
pub struct SkipMap<K, V> {
    inner: base::SkipList<K, V>,
}

impl<K, V> SkipMap<K, V> {
    /// Returns a new, empty map.
    pub fn new() -> SkipMap<K, V> {
        SkipMap {
            inner: base::SkipList::new(epoch::default_collector().clone()),
        }
    }

    /// Returns `true` if the map is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns the number of entries in the map.
    ///
    /// If the map is being concurrently modified, consider the returned number just an
    /// approximation without any guarantees.
    pub fn len(&self) -> usize {
        self.inner.len()
    }
}

impl<K, V> SkipMap<K, V>
where
    K: Ord,
{
    /// Returns the entry with the smallest key.
    pub fn front(&self) -> Option<Entry<K, V>> {
        let guard = &epoch::pin();
        try_pin_loop(|| self.inner.front(guard)).map(Entry::new)
    }

    /// Returns the entry with the largest key.
    pub fn back(&self) -> Option<Entry<K, V>> {
        let guard = &epoch::pin();
        try_pin_loop(|| self.inner.back(guard)).map(Entry::new)
    }

    /// Returns `true` if the map contains a value for the specified key.
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let guard = &epoch::pin();
        self.inner.contains_key(key, guard)
    }

    /// Returns an entry with the specified `key`.
    pub fn get<Q>(&self, key: &Q) -> Option<Entry<K, V>>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let guard = &epoch::pin();
        try_pin_loop(|| self.inner.get(key, guard)).map(Entry::new)
    }

    /// Returns an `Entry` pointing to the lowest element whose key is above
    /// the given bound. If no such element is found then `None` is
    /// returned.
    pub fn lower_bound<'a, Q>(&'a self, bound: Bound<&Q>) -> Option<Entry<'a, K, V>>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let guard = &epoch::pin();
        try_pin_loop(|| self.inner.lower_bound(bound, guard)).map(Entry::new)
    }

    /// Returns an `Entry` pointing to the highest element whose key is below
    /// the given bound. If no such element is found then `None` is
    /// returned.
    pub fn upper_bound<'a, Q>(&'a self, bound: Bound<&Q>) -> Option<Entry<'a, K, V>>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let guard = &epoch::pin();
        try_pin_loop(|| self.inner.upper_bound(bound, guard)).map(Entry::new)
    }

    /// Finds an entry with the specified key, or inserts a new `key`-`value` pair if none exist.
    pub fn get_or_insert(&self, key: K, value: V) -> Entry<K, V> {
        let guard = &epoch::pin();
        Entry::new(self.inner.get_or_insert(key, value, guard))
    }

    /// Returns an iterator over all entries in the map.
    pub fn iter(&self) -> Iter<K, V> {
        Iter {
            inner: self.inner.ref_iter(),
        }
    }

    /// Returns an iterator over a subset of entries in the skip list.
    pub fn range<'a, 'k, Min, Max>(
        &'a self,
        lower_bound: Bound<&'k Min>,
        upper_bound: Bound<&'k Max>,
    ) -> Range<'a, 'k, Min, Max, K, V>
    where
        K: Ord + Borrow<Min> + Borrow<Max>,
        Min: Ord + ?Sized + 'k,
        Max: Ord + ?Sized + 'k,
    {
        Range {
            inner: self.inner.ref_range(lower_bound, upper_bound),
        }
    }
}

impl<K, V> SkipMap<K, V>
where
    K: Ord + Send + 'static,
    V: Send + 'static,
{
    /// Inserts a `key`-`value` pair into the map and returns the new entry.
    ///
    /// If there is an existing entry with this key, it will be removed before inserting the new
    /// one.
    pub fn insert(&self, key: K, value: V) -> Entry<K, V> {
        let guard = &epoch::pin();
        Entry::new(self.inner.insert(key, value, guard))
    }

    /// Removes an entry with the specified `key` from the map and returns it.
    pub fn remove<Q>(&self, key: &Q) -> Option<Entry<K, V>>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let guard = &epoch::pin();
        self.inner.remove(key, guard).map(Entry::new)
    }

    /// Removes an entry from the front of the map.
    pub fn pop_front(&self) -> Option<Entry<K, V>> {
        let guard = &epoch::pin();
        self.inner.pop_front(guard).map(Entry::new)
    }

    /// Removes an entry from the back of the map.
    pub fn pop_back(&self) -> Option<Entry<K, V>> {
        let guard = &epoch::pin();
        self.inner.pop_back(guard).map(Entry::new)
    }

    /// Iterates over the map and removes every entry.
    pub fn clear(&self) {
        let guard = &mut epoch::pin();
        self.inner.clear(guard);
    }
}

impl<K, V> Default for SkipMap<K, V> {
    fn default() -> SkipMap<K, V> {
        SkipMap::new()
    }
}

impl<K, V> fmt::Debug for SkipMap<K, V>
where
    K: Ord + fmt::Debug,
    V: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut m = f.debug_map();
        for e in self.iter() {
            m.entry(e.key(), e.value());
        }
        m.finish()
    }
}

impl<K, V> IntoIterator for SkipMap<K, V> {
    type Item = (K, V);
    type IntoIter = IntoIter<K, V>;

    fn into_iter(self) -> IntoIter<K, V> {
        IntoIter {
            inner: self.inner.into_iter(),
        }
    }
}

impl<'a, K, V> IntoIterator for &'a SkipMap<K, V>
where
    K: Ord,
{
    type Item = Entry<'a, K, V>;
    type IntoIter = Iter<'a, K, V>;

    fn into_iter(self) -> Iter<'a, K, V> {
        self.iter()
    }
}

impl<K, V> FromIterator<(K, V)> for SkipMap<K, V>
where
    K: Ord,
{
    fn from_iter<I>(iter: I) -> SkipMap<K, V>
    where
        I: IntoIterator<Item = (K, V)>,
    {
        let s = SkipMap::new();
        for (k, v) in iter {
            s.get_or_insert(k, v);
        }
        s
    }
}

/// A reference-counted entry in a map.
pub struct Entry<'a, K: 'a, V: 'a> {
    inner: base::RefEntry<'a, K, V>,
}

impl<'a, K, V> Entry<'a, K, V> {
    fn new(inner: base::RefEntry<'a, K, V>) -> Entry<'a, K, V> {
        Entry { inner }
    }

    /// Returns a reference to the key.
    pub fn key(&self) -> &K {
        self.inner.key()
    }

    /// Returns a reference to the value.
    pub fn value(&self) -> &V {
        self.inner.value()
    }

    /// Returns `true` if the entry is removed from the map.
    pub fn is_removed(&self) -> bool {
        self.inner.is_removed()
    }
}

impl<'a, K, V> Entry<'a, K, V>
where
    K: Ord,
{
    /// Moves to the next entry in the map.
    pub fn move_next(&mut self) -> bool {
        let guard = &epoch::pin();
        self.inner.move_next(guard)
    }

    /// Moves to the previous entry in the map.
    pub fn move_prev(&mut self) -> bool {
        let guard = &epoch::pin();
        self.inner.move_prev(guard)
    }

    /// Returns the next entry in the map.
    pub fn next(&self) -> Option<Entry<'a, K, V>> {
        let guard = &epoch::pin();
        self.inner.next(guard).map(Entry::new)
    }

    /// Returns the previous entry in the map.
    pub fn prev(&self) -> Option<Entry<'a, K, V>> {
        let guard = &epoch::pin();
        self.inner.prev(guard).map(Entry::new)
    }
}

impl<'a, K, V> Entry<'a, K, V>
where
    K: Ord + Send + 'static,
    V: Send + 'static,
{
    /// Removes the entry from the map.
    ///
    /// Returns `true` if this call removed the entry and `false` if it was already removed.
    pub fn remove(&self) -> bool {
        let guard = &epoch::pin();
        self.inner.remove(guard)
    }
}

impl<'a, K, V> Clone for Entry<'a, K, V> {
    fn clone(&self) -> Entry<'a, K, V> {
        Entry {
            inner: self.inner.clone(),
        }
    }
}

impl<'a, K, V> fmt::Debug for Entry<'a, K, V>
where
    K: fmt::Debug,
    V: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("Entry")
            .field(self.key())
            .field(self.value())
            .finish()
    }
}

/// An owning iterator over the entries of a `SkipMap`.
pub struct IntoIter<K, V> {
    inner: base::IntoIter<K, V>,
}

impl<K, V> Iterator for IntoIter<K, V> {
    type Item = (K, V);

    fn next(&mut self) -> Option<(K, V)> {
        self.inner.next()
    }
}

impl<K, V> fmt::Debug for IntoIter<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "IntoIter {{ ... }}")
    }
}

/// An iterator over the entries of a `SkipMap`.
pub struct Iter<'a, K: 'a, V: 'a> {
    inner: base::RefIter<'a, K, V>,
}

impl<'a, K, V> Iterator for Iter<'a, K, V>
where
    K: Ord,
{
    type Item = Entry<'a, K, V>;

    fn next(&mut self) -> Option<Entry<'a, K, V>> {
        let guard = &epoch::pin();
        self.inner.next(guard).map(Entry::new)
    }
}

impl<'a, K, V> DoubleEndedIterator for Iter<'a, K, V>
where
    K: Ord,
{
    fn next_back(&mut self) -> Option<Entry<'a, K, V>> {
        let guard = &epoch::pin();
        self.inner.next_back(guard).map(Entry::new)
    }
}

impl<'a, K, V> fmt::Debug for Iter<'a, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Iter {{ ... }}")
    }
}

/// An iterator over the entries of a `SkipMap`.
pub struct Range<'a, 'k, Min, Max, K: 'a, V: 'a>
where
    K: Ord + Borrow<Min> + Borrow<Max>,
    Min: Ord + ?Sized + 'k,
    Max: Ord + ?Sized + 'k,
{
    inner: base::RefRange<'a, 'k, Min, Max, K, V>,
}

impl<'a, 'k, Min, Max, K, V> Iterator for Range<'a, 'k, Min, Max, K, V>
where
    K: Ord + Borrow<Min> + Borrow<Max>,
    Min: Ord + ?Sized + 'k,
    Max: Ord + ?Sized + 'k,
{
    type Item = Entry<'a, K, V>;

    fn next(&mut self) -> Option<Entry<'a, K, V>> {
        let guard = &epoch::pin();
        self.inner.next(guard).map(Entry::new)
    }
}

impl<'a, 'k, Min, Max, K, V> DoubleEndedIterator for Range<'a, 'k, Min, Max, K, V>
where
    K: Ord + Borrow<Min> + Borrow<Max>,
    Min: Ord + ?Sized + 'k,
    Max: Ord + ?Sized + 'k,
{
    fn next_back(&mut self) -> Option<Entry<'a, K, V>> {
        let guard = &epoch::pin();
        self.inner.next_back(guard).map(Entry::new)
    }
}

impl<'a, 'k, Min, Max, K, V> fmt::Debug for Range<'a, 'k, Min, Max, K, V>
where
    K: Ord + Borrow<Min> + Borrow<Max>,
    Min: Ord + ?Sized + 'k,
    Max: Ord + ?Sized + 'k,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Range {{ ... }}")
    }
}

#[cfg(test)]
mod tests {
    use super::SkipMap;

    #[test]
    fn smoke() {
        let m = SkipMap::new();
        m.insert(1, 10);
        m.insert(5, 50);
        m.insert(7, 70);
    }

    #[test]
    fn iter() {
        let s = SkipMap::new();
        for &x in &[4, 2, 12, 8, 7, 11, 5] {
            s.insert(x, x * 10);
        }

        assert_eq!(
            s.iter().map(|e| *e.key()).collect::<Vec<_>>(),
            &[2, 4, 5, 7, 8, 11, 12]
        );

        let mut it = s.iter();
        s.remove(&2);
        assert_eq!(*it.next().unwrap().key(), 4);
        s.remove(&7);
        assert_eq!(*it.next().unwrap().key(), 5);
        s.remove(&5);
        assert_eq!(*it.next().unwrap().key(), 8);
        s.remove(&12);
        assert_eq!(*it.next().unwrap().key(), 11);
        assert!(it.next().is_none());
    }

    #[test]
    fn iter_range() {
        use Bound::*;
        let s = SkipMap::new();
        let v = (0..10).map(|x| x * 10).collect::<Vec<_>>();
        for &x in v.iter() {
            s.insert(x, x);
        }

        assert_eq!(
            s.iter().map(|x| *x.value()).collect::<Vec<_>>(),
            vec![0, 10, 20, 30, 40, 50, 60, 70, 80, 90]
        );
        assert_eq!(
            s.iter().rev().map(|x| *x.value()).collect::<Vec<_>>(),
            vec![90, 80, 70, 60, 50, 40, 30, 20, 10, 0]
        );
        assert_eq!(
            s.range(Unbounded, Unbounded)
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![0, 10, 20, 30, 40, 50, 60, 70, 80, 90]
        );

        assert_eq!(
            s.range(Included(&0), Unbounded)
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![0, 10, 20, 30, 40, 50, 60, 70, 80, 90]
        );
        assert_eq!(
            s.range(Excluded(&0), Unbounded)
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![10, 20, 30, 40, 50, 60, 70, 80, 90]
        );
        assert_eq!(
            s.range(Included(&25), Unbounded)
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![30, 40, 50, 60, 70, 80, 90]
        );
        assert_eq!(
            s.range(Excluded(&25), Unbounded)
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![30, 40, 50, 60, 70, 80, 90]
        );
        assert_eq!(
            s.range(Included(&70), Unbounded)
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![70, 80, 90]
        );
        assert_eq!(
            s.range(Excluded(&70), Unbounded)
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![80, 90]
        );
        assert_eq!(
            s.range(Included(&100), Unbounded)
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![]
        );
        assert_eq!(
            s.range(Excluded(&100), Unbounded)
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![]
        );

        assert_eq!(
            s.range(Unbounded, Included(&90))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![0, 10, 20, 30, 40, 50, 60, 70, 80, 90]
        );
        assert_eq!(
            s.range(Unbounded, Excluded(&90))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![0, 10, 20, 30, 40, 50, 60, 70, 80]
        );
        assert_eq!(
            s.range(Unbounded, Included(&25))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![0, 10, 20]
        );
        assert_eq!(
            s.range(Unbounded, Excluded(&25))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![0, 10, 20]
        );
        assert_eq!(
            s.range(Unbounded, Included(&70))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![0, 10, 20, 30, 40, 50, 60, 70]
        );
        assert_eq!(
            s.range(Unbounded, Excluded(&70))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![0, 10, 20, 30, 40, 50, 60]
        );
        assert_eq!(
            s.range(Unbounded, Included(&-1))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![]
        );
        assert_eq!(
            s.range(Unbounded, Excluded(&-1))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![]
        );

        assert_eq!(
            s.range(Included(&25), Included(&80))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![30, 40, 50, 60, 70, 80]
        );
        assert_eq!(
            s.range(Included(&25), Excluded(&80))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![30, 40, 50, 60, 70]
        );
        assert_eq!(
            s.range(Excluded(&25), Included(&80))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![30, 40, 50, 60, 70, 80]
        );
        assert_eq!(
            s.range(Excluded(&25), Excluded(&80))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![30, 40, 50, 60, 70]
        );

        assert_eq!(
            s.range(Included(&25), Included(&25))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![]
        );
        assert_eq!(
            s.range(Included(&25), Excluded(&25))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![]
        );
        assert_eq!(
            s.range(Excluded(&25), Included(&25))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![]
        );
        assert_eq!(
            s.range(Excluded(&25), Excluded(&25))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![]
        );

        assert_eq!(
            s.range(Included(&50), Included(&50))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![50]
        );
        assert_eq!(
            s.range(Included(&50), Excluded(&50))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![]
        );
        assert_eq!(
            s.range(Excluded(&50), Included(&50))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![]
        );
        assert_eq!(
            s.range(Excluded(&50), Excluded(&50))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![]
        );

        assert_eq!(
            s.range(Included(&100), Included(&-2))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![]
        );
        assert_eq!(
            s.range(Included(&100), Excluded(&-2))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![]
        );
        assert_eq!(
            s.range(Excluded(&100), Included(&-2))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![]
        );
        assert_eq!(
            s.range(Excluded(&100), Excluded(&-2))
                .map(|x| *x.value())
                .collect::<Vec<_>>(),
            vec![]
        );
    }

    // TODO(stjepang): Write more tests.
}
