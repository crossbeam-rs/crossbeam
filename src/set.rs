use std::borrow::Borrow;
use std::fmt;
use std::iter::FromIterator;

use base;

/// A set based on a lock-free skip list.
pub struct SkipSet<T> {
    inner: base::SkipList<T, ()>,
}

impl<T> SkipSet<T> {
    /// Returns a new, empty set.
    pub fn new() -> SkipSet<T> {
        SkipSet {
            inner: base::SkipList::new(),
        }
    }

    /// Returns `true` if the set is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns the number of entries in the set.
    ///
    /// If the set is being concurrently modified, consider the returned number just an
    /// approximation without any guarantees.
    pub fn len(&self) -> usize {
        self.inner.len()
    }
}

impl<T> SkipSet<T>
where
    T: Ord,
{
    /// Returns the entry with the smallest key.
    pub fn front(&self) -> Option<Entry<T>> {
        self.inner.front().map(Entry::new)
    }

    /// Returns the entry with the largest key.
    pub fn back(&self) -> Option<Entry<T>> {
        self.inner.back().map(Entry::new)
    }

    /// Returns `true` if the set contains a value for the specified key.
    pub fn contains<Q>(&self, key: &Q) -> bool
    where
        T: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.get(key).is_some()
    }

    /// Returns an entry with the specified `key`.
    pub fn get<Q>(&self, key: &Q) -> Option<Entry<T>>
    where
        T: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.inner.get(key).map(Entry::new)
    }

    /// Returns the first entry with a key greater than or equal to `key`, or `None` if all entries
    /// have smaller keys.
    pub fn seek<Q>(&self, key: &Q) -> Option<Entry<T>>
    where
        T: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.inner.seek(key).map(Entry::new)
    }

    /// Finds an entry with the specified key, or inserts a new `key`-`value` pair if none exist.
    pub fn get_or_insert(&self, key: T) -> Entry<T> {
        Entry::new(self.inner.get_or_insert(key, ()))
    }

    /// Returns an iterator over all entries in the map.
    pub fn iter(&self) -> Iter<T> {
        Iter {
            inner: self.inner.iter(),
        }
    }

    // TODO(stjepang): Add `fn range`.
}

impl<T> SkipSet<T>
where
    T: Ord + Send + 'static,
{
    /// Inserts a `key`-`value` pair into the set and returns the new entry.
    ///
    /// If there is an existing entry with this key, it will be removed before inserting the new
    /// one.
    pub fn insert(&self, key: T) -> Entry<T> {
        Entry::new(self.inner.insert(key, ()))
    }

    /// Removes an entry with the specified key from the set and returns it.
    pub fn remove<Q>(&self, key: &Q) -> Option<Entry<T>>
    where
        T: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.inner.remove(key).map(Entry::new)
    }

    /// Removes an entry from the front of the map.
    pub fn pop_front(&self) -> Option<Entry<T>> {
        self.inner.pop_front().map(Entry::new)
    }

    /// Removes an entry from the back of the map.
    pub fn pop_back(&self) -> Option<Entry<T>> {
        self.inner.pop_back().map(Entry::new)
    }

    /// Iterates over the set and removes every entry.
    pub fn clear(&self) {
        self.inner.clear();
    }
}

impl<T> Default for SkipSet<T> {
    fn default() -> SkipSet<T> {
        SkipSet::new()
    }
}

impl<T> fmt::Debug for SkipSet<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "SkipSet {{ ... }}")
    }
}

impl<T> IntoIterator for SkipSet<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> IntoIter<T> {
        IntoIter {
            inner: self.inner.into_iter(),
        }
    }
}

impl<'a, T> IntoIterator for &'a SkipSet<T>
where
    T: Ord,
{
    type Item = Entry<'a, T>;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

impl<T> FromIterator<T> for SkipSet<T>
where
    T: Ord,
{
    fn from_iter<I>(iter: I) -> SkipSet<T>
    where
        I: IntoIterator<Item = T>,
    {
        let s = SkipSet::new();
        for t in iter {
            s.get_or_insert(t);
        }
        s
    }
}

pub struct Entry<'a, T: 'a> {
    inner: base::Entry<'a, T, ()>,
}

unsafe impl<'a, T: Send + Sync> Send for Entry<'a, T> {}
unsafe impl<'a, T: Send + Sync> Sync for Entry<'a, T> {}

impl<'a, T> Entry<'a, T> {
    fn new(inner: base::Entry<'a, T, ()>) -> Entry<'a, T> {
        Entry { inner }
    }

    /// Returns a reference to the key.
    pub fn value(&self) -> &T {
        self.inner.key()
    }

    /// Returns `true` if the entry is removed from the set.
    pub fn is_removed(&self) -> bool {
        self.inner.is_removed()
    }
}

impl<'a, T> Entry<'a, T>
where
    T: Ord,
{
    pub fn next(&mut self) -> bool {
        self.inner.next()
    }

    pub fn prev(&mut self) -> bool {
        self.inner.prev()
    }

    /// Returns the next entry in the set.
    pub fn get_next(&self) -> Option<Entry<'a, T>> {
        self.inner.get_next().map(Entry::new)
    }

    /// Returns the previous entry in the set.
    pub fn get_prev(&self) -> Option<Entry<'a, T>> {
        self.inner.get_prev().map(Entry::new)
    }
}

impl<'a, T> Entry<'a, T>
where
    T: Ord + Send + 'static,
{
    /// Removes the entry from the set.
    ///
    /// Returns `true` if this call removed the entry and `false` if it was already removed.
    pub fn remove(&self) -> bool {
        self.inner.remove()
    }
}

impl<'a, T> Clone for Entry<'a, T> {
    fn clone(&self) -> Entry<'a, T> {
        Entry {
            inner: self.inner.clone(),
        }
    }
}

impl<'a, T> fmt::Debug for Entry<'a, T>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("Entry")
            .field(&self.value())
            .finish()
    }
}

/// An owning iterator over the entries of a `SkipSet`.
pub struct IntoIter<T> {
    inner: base::IntoIter<T, ()>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.inner.next().map(|(k, ())| k)
    }
}

impl<T> fmt::Debug for IntoIter<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "IntoIter {{ ... }}")
    }
}

/// An iterator over the entries of a `SkipSet`.
pub struct Iter<'a, T: 'a> {
    inner: base::Iter<'a, T, ()>,
}

impl<'a, T> Iterator for Iter<'a, T>
where
    T: Ord,
{
    type Item = Entry<'a, T>;

    fn next(&mut self) -> Option<Entry<'a, T>> {
        self.inner.next().map(Entry::new)
    }
}

impl<'a, T> DoubleEndedIterator for Iter<'a, T>
where
    T: Ord,
{
    fn next_back(&mut self) -> Option<Entry<'a, T>> {
        self.inner.next_back().map(Entry::new)
    }
}

impl<'a, T> fmt::Debug for Iter<'a, T>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Iter {{ ... }}")
    }
}

#[cfg(test)]
mod tests {
    use super::SkipSet;

    #[test]
    fn smoke() {
        let m = SkipSet::new();
        m.insert(1);
        m.insert(5);
        m.insert(7);
    }

    // TODO(stjepang): Write more tests.
}
