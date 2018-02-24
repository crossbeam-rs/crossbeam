use std::borrow::Borrow;
use std::fmt;
use std::iter::FromIterator;

use base;

/// A map based on a lock-free skip list.
pub struct SkipMap<K, V> {
    inner: base::SkipList<K, V>,
}

impl<K, V> SkipMap<K, V> {
    /// Returns a new, empty map.
    pub fn new() -> SkipMap<K, V> {
        SkipMap {
            inner: base::SkipList::new(),
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
        self.inner.front().map(Entry::new)
    }

    /// Returns the entry with the largest key.
    pub fn back(&self) -> Option<Entry<K, V>> {
        self.inner.back().map(Entry::new)
    }

    /// Returns `true` if the map contains a value for the specified key.
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.get(key).is_some()
    }

    /// Returns an entry with the specified `key`.
    pub fn get<Q>(&self, key: &Q) -> Option<Entry<K, V>>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.inner.get(key).map(Entry::new)
    }

    /// Returns the first entry with a key greater than or equal to `key`, or `None` if all entries
    /// have smaller keys.
    pub fn seek<Q>(&self, key: &Q) -> Option<Entry<K, V>>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.inner.seek(key).map(Entry::new)
    }

    /// Finds an entry with the specified key, or inserts a new `key`-`value` pair if none exist.
    pub fn get_or_insert(&self, key: K, value: V) -> Entry<K, V> {
        Entry::new(self.inner.get_or_insert(key, value))
    }

    /// Returns an iterator over all entries in the map.
    pub fn iter(&self) -> Iter<K, V> {
        Iter {
            inner: self.inner.iter(),
        }
    }

    // TODO(stjepang): Add `fn range`.
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
        Entry::new(self.inner.insert(key, value))
    }

    /// Removes an entry with the specified `key` from the map and returns it.
    pub fn remove<Q>(&self, key: &Q) -> Option<Entry<K, V>>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.inner.remove(key).map(Entry::new)
    }

    /// Removes an entry from the front of the map.
    pub fn pop_front(&self) -> Option<Entry<K, V>> {
        self.inner.pop_front().map(Entry::new)
    }

    /// Removes an entry from the back of the map.
    pub fn pop_back(&self) -> Option<Entry<K, V>> {
        self.inner.pop_back().map(Entry::new)
    }

    /// Iterates over the map and removes every entry.
    pub fn clear(&self) {
        self.inner.clear();
    }
}

impl<K, V> Default for SkipMap<K, V> {
    fn default() -> SkipMap<K, V> {
        SkipMap::new()
    }
}

impl<K, V> fmt::Debug for SkipMap<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "SkipMap {{ ... }}")
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
    inner: base::Entry<'a, K, V>,
}

unsafe impl<'a, K: Send + Sync, V: Send + Sync> Send for Entry<'a, K, V> {}
unsafe impl<'a, K: Send + Sync, V: Send + Sync> Sync for Entry<'a, K, V> {}

impl<'a, K, V> Entry<'a, K, V> {
    fn new(inner: base::Entry<'a, K, V>) -> Entry<'a, K, V> {
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

    // TODO(stjepang): Add `fn try_into_value(self)`.
}

impl<'a, K, V> Entry<'a, K, V>
where
    K: Ord,
{
    /// Moves to the next entry in the map.
    pub fn next(&mut self) -> bool {
        self.inner.next()
    }

    /// Moves to the previous entry in the map.
    pub fn prev(&mut self) -> bool {
        self.inner.prev()
    }

    /// Returns the next entry in the map.
    pub fn get_next(&self) -> Option<Entry<'a, K, V>> {
        self.inner.get_next().map(Entry::new)
    }

    /// Returns the previous entry in the map.
    pub fn get_prev(&self) -> Option<Entry<'a, K, V>> {
        self.inner.get_prev().map(Entry::new)
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
        self.inner.remove()
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
            .field(&self.key())
            .field(&self.value())
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

impl<K, V> fmt::Debug for IntoIter<K, V>
where
    K: fmt::Debug,
    V: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "IntoIter {{ ... }}")
    }
}

/// An iterator over the entries of a `SkipMap`.
pub struct Iter<'a, K: 'a, V: 'a> {
    inner: base::Iter<'a, K, V>,
}

impl<'a, K, V> Iterator for Iter<'a, K, V>
where
    K: Ord,
{
    type Item = Entry<'a, K, V>;

    fn next(&mut self) -> Option<Entry<'a, K, V>> {
        self.inner.next().map(Entry::new)
    }
}

impl<'a, K, V> DoubleEndedIterator for Iter<'a, K, V>
where
    K: Ord,
{
    fn next_back(&mut self) -> Option<Entry<'a, K, V>> {
        self.inner.next_back().map(Entry::new)
    }
}

impl<'a, K, V> fmt::Debug for Iter<'a, K, V>
where
    K: fmt::Debug,
    V: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Iter {{ ... }}")
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

    // TODO(stjepang): Write more tests.
}
