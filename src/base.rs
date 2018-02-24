use std::borrow::Borrow;
use std::cmp;
use std::mem;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use epoch::{self, Atomic, Guard, Shared};
use utils::cache_padded::CachePadded;

/// Number of bits needed to store height.
const HEIGHT_BITS: usize = 5;

/// Maximum height of a skip list tower.
const MAX_HEIGHT: usize = 1 << HEIGHT_BITS;

/// The bits of `refs_and_height` that keep the height.
const HEIGHT_MASK: usize = (1 << HEIGHT_BITS) - 1;

/// A skip list node.
///
/// This struct is marked with `repr(C)` so that the specific order of fields is enforced.
/// It is important that the tower is the last field since it is dynamically sized. The key,
/// reference count, and height are kept close to the tower to improve cache locality during
/// skip list traversal.
#[repr(C)]
pub struct Node<K, V> {
    /// The value.
    value: V,

    /// The key.
    key: K,

    /// Keeps the reference count and the height of its tower.
    ///
    /// The reference count is equal to the number of `Entry`s pointing to this node, plus the
    /// number of levels in which this node is installed.
    refs_and_height: AtomicUsize,

    /// The tower of atomic pointers.
    pointers: [Atomic<Node<K, V>>; 1],
}

impl<K, V> Node<K, V> {
    /// Allocates a node.
    ///
    /// The returned node will start with reference count of zero and the tower will be initialized
    /// with null pointers. However, the key and the value will be left uninitialized, and that is
    /// why this function is unsafe.
    unsafe fn alloc(height: usize) -> *mut Self {
        // TODO(stjepang): Use the new alloc API instead of this hack once it becomes stable.
        let cap = Self::size_in_u64s(height);
        let mut v = Vec::<u64>::with_capacity(cap);
        let ptr = v.as_mut_ptr() as *mut Self;
        mem::forget(v);

        ptr::write(&mut (*ptr).refs_and_height, AtomicUsize::new(height - 1));
        ptr::write_bytes(&mut (*ptr).pointers[0], 0, height);
        ptr
    }

    /// Deallocates a node.
    ///
    /// This function will not run any destructors.
    unsafe fn dealloc(ptr: *mut Self) {
        let height = (*ptr).height();
        let cap = Self::size_in_u64s(height);
        drop(Vec::from_raw_parts(ptr as *mut u64, 0, cap));
    }

    /// Returns the size of a node with tower of given `height` measured in `u64`s.
    fn size_in_u64s(height: usize) -> usize {
        assert!(1 <= height && height <= MAX_HEIGHT);
        assert!(mem::align_of::<Self>() <= mem::align_of::<u64>());

        let size_base = mem::size_of::<Self>();
        let size_ptr = mem::size_of::<Atomic<Self>>();

        let size_u64 = mem::size_of::<u64>();
        let size_self = size_base + size_ptr * (height - 1);

        (size_self + size_u64 - 1) / size_u64
    }

    /// Returns the height of this node's tower.
    #[inline]
    fn height(&self) -> usize {
        (self.refs_and_height.load(Ordering::Relaxed) & HEIGHT_MASK) + 1
    }

    /// Returns a reference to the pointer at the specified `level` in the tower.
    ///
    /// Argument `level` must be in the range `0..self.height()`.
    #[inline]
    unsafe fn tower(&self, level: usize) -> &Atomic<Self> {
        self.pointers.get_unchecked(level)
    }

    /// Marks all pointers in the tower and returns `true` if the level 0 was not marked.
    fn mark_tower(&self) -> bool {
        let height = self.height();

        for level in (0..height).rev() {
            let tag = unsafe {
                // We're loading the pointer only for the tag, so it's okay to use
                // `epoch::unprotected()` in this situation.
                self.tower(level)
                    .fetch_or(1, Ordering::SeqCst, epoch::unprotected())
                    .tag()
            };

            // If the level 0 pointer was already marked, somebody else removed the node.
            if level == 0 && tag == 1 {
                return false;
            }
        }

        // We marked the level 0 pointer, therefore we removed the node.
        true
    }

    /// Returns `true` if the node is removed.
    #[inline]
    fn is_removed(&self) -> bool {
        let tag = unsafe {
            // We're loading the pointer only for the tag, so it's okay to use
            // `epoch::unprotected()` in this situation.
            self.tower(0)
                .load(Ordering::SeqCst, epoch::unprotected())
                .tag()
        };
        tag == 1
    }

    /// Attempts to increment the reference count of a node and returns `true` on success.
    ///
    /// The reference count can be incremented only if it is non-zero.
    #[inline]
    unsafe fn increment(ptr: *const Self) -> bool {
        let mut refs_and_height = (*ptr).refs_and_height.load(Ordering::Relaxed);

        loop {
            // If the reference count is zero, return `false`.
            if refs_and_height & !HEIGHT_MASK == 0 {
                return false;
            }

            // If all bits in the reference count are ones, we're about to overflow it.
            if refs_and_height & !HEIGHT_MASK == !HEIGHT_MASK {
                panic!("reference count overflow");
            }

            // Try incrementing the count.
            match (*ptr).refs_and_height.compare_exchange_weak(
                refs_and_height,
                refs_and_height + (1 << HEIGHT_BITS),
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(current) => refs_and_height = current,
            }
        }
    }

    /// Decrements the reference count of a node, destroying it if the count becomes zero.
    #[inline]
    unsafe fn decrement(ptr: *const Self) {
        if (*ptr)
            .refs_and_height
            .fetch_sub(1 << HEIGHT_BITS, Ordering::AcqRel) >> HEIGHT_BITS == 1
        {
            Self::finalize(ptr);
        }
    }

    /// Drops the value of a node, then defers destruction of the key and deallocation.
    #[cold]
    unsafe fn finalize(ptr: *const Self) {
        epoch::pin().defer(move || {
            let ptr = ptr as *mut Self;

            // Call destructors: drop the key and the value.
            ptr::drop_in_place(&mut (*ptr).key);
            ptr::drop_in_place(&mut (*ptr).value);

            // Finally, deallocate the memory occupied by the node.
            Node::dealloc(ptr);
        });
    }
}

/// Frequently modified data associated with a skip list.
struct HotData {
    /// The seed for random height generation.
    seed: AtomicUsize,

    /// The number of entries in the skip list.
    len: AtomicUsize,
}

/// A lock-free skip list.
// TODO(stjepang): Embed a custom `epoch::Collector` inside `SkipList<K, V>`. Instead of adding
// garbage to the default global collector, we should add it to a local collector tied to the
// particular skip list instance.
//
// Since global collector might destroy garbage arbitrarily late in the future, some skip list
// methods have `K: 'static` and `V: 'static` bounds. But a local collector embedded in the skip
// list would destroy all remaining garbage when the skip list is dropped, so in that case we'd be
// able to remove those bounds on types `K` and `V`.
//
// As a further future optimization, if `!mem::needs_drop::<K>() && !mem::needs_drop::<V>()`
// (neither key nor the value have destructors), there's no point in creating a new local
// collector, so we should simply use the global one.
pub struct SkipList<K, V> {
    /// The head of the skip list (just a dummy node, not a real entry).
    head: *const Node<K, V>,

    /// Hot data associated with the skip list, stored in a dedicated cache line.
    hot_data: Box<CachePadded<HotData>>,
}

unsafe impl<K: Send + Sync, V: Send + Sync> Send for SkipList<K, V> {}
unsafe impl<K: Send + Sync, V: Send + Sync> Sync for SkipList<K, V> {}

impl<K, V> SkipList<K, V> {
    /// Returns a new, empty skip list.
    pub fn new() -> SkipList<K, V> {
        SkipList {
            head: unsafe { Node::alloc(MAX_HEIGHT) },
            hot_data: Box::new(CachePadded::new(HotData {
                seed: AtomicUsize::new(1),
                len: AtomicUsize::new(0),
            })),
        }
    }

    /// Returns `true` if the skip list is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the number of entries in the skip list.
    ///
    /// If the skip list is being concurrently modified, consider the returned number just an
    /// approximation without any guarantees.
    pub fn len(&self) -> usize {
        self.hot_data.len.load(Ordering::SeqCst)
    }
}

impl<K, V> SkipList<K, V>
where
    K: Ord,
{
    /// Returns the entry with the smallest key.
    pub fn front(&self) -> Option<Entry<K, V>> {
        let guard = &epoch::pin();

        loop {
            unsafe {
                // Load the level 0 successor of the head.
                let head = &*self.head;
                let ptr = head.tower(0).load(Ordering::SeqCst, guard);

                match ptr.as_ref() {
                    None => return None,
                    Some(n) => {
                        // If this node is removed, search for its key in order to unlink it.
                        if n.is_removed() {
                            self.search(Some(&n.key), guard);
                        // If we can increment the reference count, return the entry.
                        } else if Node::increment(n) {
                            return Some(Entry::from_raw(self, n));
                        }
                    }
                }
            }
        }
    }

    /// Returns the entry with the largest key.
    pub fn back(&self) -> Option<Entry<K, V>> {
        let guard = &epoch::pin();

        loop {
            let search = self.search(None, guard);
            let node = search.left[0];

            unsafe {
                // If the last node is the head, the skip list is empty.
                if ptr::eq(node, self.head) {
                    return None;
                // If we can increment the reference count, return the entry.
                } else if Node::increment(node) {
                    return Some(Entry::from_raw(self, node));
                }
            }
        }
    }

    /// Generates a random height and returns it.
    fn random_height(&self) -> usize {
        // Pseudorandom number generation from "Xorshift RNGs" by George Marsaglia.
        //
        // This particular set of operations generates 32-bit integers. See:
        // https://en.wikipedia.org/wiki/Xorshift#Example_implementation
        let mut num = self.hot_data.seed.load(Ordering::Relaxed);
        num ^= num << 13;
        num ^= num >> 17;
        num ^= num << 5;
        self.hot_data.seed.store(num, Ordering::Relaxed);

        let mut height = cmp::min(MAX_HEIGHT, num.trailing_zeros() as usize + 1);
        unsafe {
            // Keep decreasing the height while it's much larger than all towers currently in the
            // skip list.
            //
            // Note that we're loading the pointer only to check whether it is null, so it's okay
            // to use `epoch::unprotected()` in this situation.
            while height >= 4
                && (*self.head)
                    .tower(height - 2)
                    .load(Ordering::Relaxed, epoch::unprotected())
                    .is_null()
            {
                height -= 1;
            }
        }
        height
    }

    /// Searches for a key in the skip list.
    ///
    /// If `key` is `None`, this method will search for an infinitely large key and reach the end
    /// of the skip list.
    fn search<'g, Q>(&self, key: Option<&Q>, guard: &'g Guard) -> Search<'g, K, V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        unsafe {
            'search: loop {
                // The result of this search.
                let mut search = Search {
                    found: false,
                    left: [&*self.head; MAX_HEIGHT],
                    right: [Shared::null(); MAX_HEIGHT],
                };

                // The current level we're at.
                let mut level = MAX_HEIGHT;

                // Go down until we find a level at least one tower can actually reach.
                while level >= 1
                    && (*self.head)
                        .tower(level - 1)
                        .load(Ordering::SeqCst, guard)
                        .is_null()
                {
                    level -= 1;
                }

                // The current node we're at.
                let mut node = &*self.head;

                while level >= 1 {
                    level -= 1;

                    // Two adjacent nodes at the current level.
                    let mut pred = node;
                    let mut curr = pred.tower(level).load(Ordering::SeqCst, guard);

                    // If `curr` is marked, that means `pred` is removed and we have to restart the
                    // search.
                    if curr.tag() == 1 {
                        continue 'search;
                    }

                    // Iterate through the current level until we reach a node with a key greater
                    // than or equal to `key`.
                    while let Some(c) = curr.as_ref() {
                        let succ = c.tower(level).load(Ordering::SeqCst, guard);

                        if succ.tag() == 1 {
                            // If `succ` is marked, that means `curr` is removed. Let's try
                            // unlinking it from the skip list at this level.
                            match pred.tower(level).compare_and_set(
                                curr,
                                succ.with_tag(0),
                                Ordering::SeqCst,
                                guard,
                            ) {
                                Ok(_) => {
                                    // On success, decrement the reference count of `curr` and
                                    // continue searching through the current level.
                                    Node::decrement(curr.as_raw());
                                    curr = succ.with_tag(0);
                                    continue;
                                }
                                Err(_) => {
                                    // On failure, we cannot do anything reasonable to continue
                                    // searching from the current position. Restart the search.
                                    continue 'search;
                                }
                            }
                        }

                        // If `curr` contains a key that is greater than or equal to `key`, we're
                        // done with this level.
                        if key.map(|k| c.key.borrow() >= k) == Some(true) {
                            break;
                        }

                        // Move one step forward.
                        pred = c;
                        curr = succ;
                    }

                    // Store the position at the current level into the result.
                    search.left[level] = pred;
                    search.right[level] = curr;

                    node = pred;
                }

                // Check if we have found a node with key equal to `key`.
                search.found = search.right[0]
                    .as_ref()
                    .map(|r| Some(r.key.borrow()) == key)
                    == Some(true);

                return search;
            }
        }
    }

    /// Inserts an entry with the specified `key` and `value`.
    ///
    /// If `replace` is `true`, then any existing entry with this key will first be removed.
    fn insert_internal(&self, key: K, value: V, replace: bool) -> Entry<K, V> {
        let guard = &epoch::pin();
        let mut search;

        unsafe {
            loop {
                // First try searching for the key.
                // Note that the `Ord` implementation for `K` may panic during the search.
                search = self.search(Some(&key), guard);

                if !search.found {
                    break;
                }

                let r = search.right[0];

                if replace {
                    // If a node with the key was found and we should replace it, mark its tower
                    // and then repeat the search.
                    if r.deref().mark_tower() {
                        self.hot_data.len.fetch_sub(1, Ordering::SeqCst);
                    }
                } else {
                    // If a node with the key was found and we're not going to replace it, let's
                    // try returning it as an entry.
                    if Node::increment(r.as_raw()) {
                        return Entry::from_raw(self, r.as_raw());
                    }

                    // If we couldn't increment the reference count, that means someone has just
                    // now removed the node.
                    break;
                }
            }

            // Create a new node.
            let height = self.random_height();
            let (node, n) = {
                let n = Node::<K, V>::alloc(height);

                // Write the key and the value into the node.
                ptr::write(&mut (*n).key, key);
                ptr::write(&mut (*n).value, value);

                // The reference count is initially zero. Let's increment it twice to account for:
                // 1. The entry that will be returned.
                // 2. The link at the level 0 of the tower.
                //
                // Use `fetch_add` to increment the refcount while keeping the height the same.
                (*n).refs_and_height
                    .fetch_add(2 << HEIGHT_BITS, Ordering::Relaxed);

                (Shared::<Node<K, V>>::from(n as *const _), &*n)
            };

            // Optimistically increment `len`.
            self.hot_data.len.fetch_add(1, Ordering::SeqCst);

            loop {
                // Set the lowest successor of `n` to `search.right[0]`.
                n.tower(0).store(search.right[0], Ordering::SeqCst);

                // Try installing the new node into the skip list (at level 0).
                if search.left[0]
                    .tower(0)
                    .compare_and_set(search.right[0], node, Ordering::SeqCst, guard)
                    .is_ok()
                {
                    break;
                }

                // We failed. Let's search for the key and try again.
                search = {
                    // Create a guard that destroys the new node in case search panics.
                    defer_on_unwind! {{
                        ptr::drop_in_place(&n.key as *const K as *mut K);
                        ptr::drop_in_place(&n.value as *const V as *mut V);
                        Node::dealloc(node.as_raw() as *mut Node<K, V>);
                    }}
                    self.search(Some(&n.key), guard)
                };

                if search.found {
                    let r = search.right[0];

                    if replace {
                        // If a node with the key was found and we should replace it, mark its
                        // tower and then repeat the search.
                        if r.deref().mark_tower() {
                            self.hot_data.len.fetch_sub(1, Ordering::SeqCst);
                        }
                    } else {
                        // If a node with the key was found and we're not going to replace it,
                        // let's try returning it as an entry.
                        if Node::increment(r.as_raw()) {
                            // Destroy the new node.
                            let raw = node.as_raw() as *mut Node<K, V>;
                            ptr::drop_in_place(&mut (*raw).key);
                            ptr::drop_in_place(&mut (*raw).value);
                            Node::dealloc(raw);
                            self.hot_data.len.fetch_sub(1, Ordering::SeqCst);

                            return Entry::from_raw(self, r.as_raw());
                        }

                        // If we couldn't increment the reference count, that means someone has
                        // just now removed the node.
                    }
                }
            }

            // The new node was successfully installed. Let's create an entry associated with it.
            let entry = Entry::from_raw(self, n);

            // Build the rest of the tower above level 0.
            'build: for level in 1..height {
                loop {
                    // Obtain the predecessor and successor at the current level.
                    let pred = search.left[level];
                    let succ = search.right[level];

                    // Load the current value of the pointer in the tower at this level.
                    let next = n.tower(level).load(Ordering::SeqCst, guard);

                    // If the current pointer is marked, that means another thread is already
                    // removing the node we've just inserted. In that case, let's just stop
                    // building the tower.
                    if next.tag() == 1 {
                        break 'build;
                    }

                    // When searching for `key` and traversing the skip list from the highest level
                    // to the lowest, it is possible to observe a node with an equal key at higher
                    // levels and then find it missing at the lower levels if it gets removed
                    // during traversal. Even worse, it is possible to observe completely different
                    // nodes with the exact same key at different levels.
                    //
                    // Linking the new node to a dead successor with an equal key could create
                    // subtle corner cases that would require special care. It's much easier to
                    // simply prohibit linking two nodes with equal keys.
                    //
                    // If the successor has the same key as the new node, that means it is marked
                    // as removed and should be unlinked from the skip list. In that case, let's
                    // repeat the search to make sure it gets unlinked and try again.
                    //
                    // If this comparison or the following search panics, we simply stop building
                    // the tower without breaking any invariants. Note that building higher levels
                    // is completely optional. Only the lowest level really matters, and all the
                    // higher levels are there just to make searching faster.
                    if succ.as_ref().map(|s| &s.key) == Some(&n.key) {
                        search = self.search(Some(&n.key), guard);
                        continue;
                    }

                    // Change the pointer at the current level from `next` to `succ`. If this CAS
                    // operation fails, that means another thread has marked the pointer and we
                    // should stop building the tower.
                    if n.tower(level)
                        .compare_and_set(next, succ, Ordering::SeqCst, guard)
                        .is_err()
                    {
                        break 'build;
                    }

                    // Increment the reference count. The current value will always be at least 1
                    // because we are holding `entry`.
                    (*n).refs_and_height
                        .fetch_add(1 << HEIGHT_BITS, Ordering::Relaxed);

                    // Try installing the new node at the current level.
                    if pred.tower(level)
                        .compare_and_set(succ, node, Ordering::SeqCst, guard)
                        .is_ok()
                    {
                        // Success! Continue on the next level.
                        break;
                    }

                    // Installation failed. Decrement the reference count.
                    (*n).refs_and_height
                        .fetch_sub(1 << HEIGHT_BITS, Ordering::Relaxed);

                    // We don't have the most up-to-date search results. Repeat the search.
                    //
                    // If this search panics, we simply stop building the tower without breaking
                    // any invariants. Note that building higher levels is completely optional.
                    // Only the lowest level really matters, and all the higher levels are there
                    // just to make searching faster.
                    search = self.search(Some(&n.key), guard);
                }
            }

            // If any pointer in the tower is marked, that means our node is in the process of
            // removal or already removed. It is possible that another thread (either partially or
            // completely) removed the new node while we were building the tower, and just after
            // that we installed the new node at one of the higher levels. In order to undo that
            // installation, we must repeat the search, which will unlink the new node at that
            // level.
            if n.tower(height - 1).load(Ordering::SeqCst, guard).tag() == 1 {
                self.search(Some(&n.key), guard);
            }

            // Finally, return the new entry.
            entry
        }
    }

    /// Finds an entry with the specified key, or inserts a new `key`-`value` pair if none exist.
    pub fn get_or_insert(&self, key: K, value: V) -> Entry<K, V> {
        self.insert_internal(key, value, false)
    }

    /// Returns an entry with the specified `key`.
    pub fn get<Q>(&self, key: &Q) -> Option<Entry<K, V>>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let guard = &epoch::pin();

        loop {
            // Search for a node with the specified key.
            let search = self.search(Some(key), guard);
            if !search.found {
                return None;
            }

            let node = search.right[0].as_raw();
            unsafe {
                // Try incrementing its reference count.
                if Node::increment(node) {
                    // Success! Return it as an entry.
                    return Some(Entry::from_raw(self, node));
                }
            }
        }
    }

    /// Returns the first entry with a key greater than or equal to `key`, or `None` if all entries
    /// have smaller keys.
    pub fn seek<Q>(&self, key: &Q) -> Option<Entry<K, V>>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let guard = &epoch::pin();

        loop {
            // Search for a node with the specified key.
            let search = self.search(Some(key), guard);
            let node = search.right[0].as_raw();

            if node.is_null() {
                return None;
            }

            // Try incrementing its reference count.
            unsafe {
                if Node::increment(node) {
                    return Some(Entry::from_raw(self, node));
                }
            }
        }
    }

    /// Returns an iterator over all entries in the skip list.
    pub fn iter(&self) -> Iter<K, V> {
        Iter {
            parent: self,
            front: None,
            back: None,
            finished: false,
        }
    }
}

impl<K, V> SkipList<K, V>
where
    K: Ord + Send + 'static,
    V: Send + 'static,
{
    /// Inserts a `key`-`value` pair into the skip list and returns the new entry.
    ///
    /// If there is an existing entry with this key, it will be removed before inserting the new
    /// one.
    pub fn insert(&self, key: K, value: V) -> Entry<K, V> {
        self.insert_internal(key, value, true)
    }

    /// Removes an entry with the specified `key` from the map and returns it.
    pub fn remove<Q>(&self, key: &Q) -> Option<Entry<K, V>>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let guard = &epoch::pin();

        loop {
            // Try searching for the key.
            let search = self.search(Some(key), guard);
            if !search.found {
                return None;
            }

            let node = search.right[0];
            unsafe {
                let n = node.deref();

                // First try incrementing the reference count because we have to return the node as
                // an entry. If this fails, repeat the search.
                if !Node::increment(n) {
                    continue;
                }

                // Create an entry.
                let entry = Entry::from_raw(self, n);

                // Try removing the node by marking its tower.
                if n.mark_tower() {
                    // Success! Decrement `len`.
                    self.hot_data.len.fetch_sub(1, Ordering::SeqCst);

                    // Unlink the node at each level of the skip list. We could do this by simply
                    // repeating the search, but it's usually faster to unlink it manually using
                    // the `left` and `right` lists.
                    for level in (0..n.height()).rev() {
                        let succ = n.tower(level).load(Ordering::SeqCst, guard).with_tag(0);

                        // Try linking the predecessor and successor at this level.
                        if search.left[level]
                            .tower(level)
                            .compare_and_set(node, succ, Ordering::SeqCst, guard)
                            .is_ok()
                        {
                            // Success! Decrement the reference count.
                            Node::decrement(n);
                        } else {
                            // Failed! Just repeat the search to completely unlink the node.
                            self.search(Some(key), guard);
                            break;
                        }
                    }

                    return Some(entry);
                }
            }
        }
    }

    /// Removes an entry from the front of the skip list.
    pub fn pop_front(&self) -> Option<Entry<K, V>> {
        loop {
            match self.front() {
                None => return None,
                Some(e) => {
                    if e.remove() {
                        return Some(e);
                    }
                }
            }
        }
    }

    /// Removes an entry from the back of the skip list.
    pub fn pop_back(&self) -> Option<Entry<K, V>> {
        loop {
            match self.back() {
                None => return None,
                Some(e) => {
                    if e.remove() {
                        return Some(e);
                    }
                }
            }
        }
    }

    /// Iterates over the map and removes every entry.
    pub fn clear(&self) {
        /// Number of steps after which we repin the current thread and unlink removed nodes.
        const BATCH_SIZE: usize = 100;

        let guard = &mut epoch::pin();
        let mut step = 0;

        if let Some(mut entry) = self.front() {
            loop {
                step += 1;
                if step == BATCH_SIZE {
                    step = 0;

                    // Repin the current thread because we don't want to keep it pinned in the same
                    // epoch for a too long time.
                    guard.repin();

                    // Search for the current entry in order to unlink all the preceeding entries
                    // we have removed.
                    //
                    // By unlinking nodes in batches we make sure that the final search doesn't
                    // unlink all nodes at once, which could keep the current thread pinned for a
                    // long time.
                    self.search(Some(entry.key()), guard);
                }

                // Before removing the current entry, first obtain the following one.
                let next = entry.get_next();

                // Try removing the current entry.
                if entry.node.mark_tower() {
                    // Success! Decrement `len`.
                    self.hot_data.len.fetch_sub(1, Ordering::SeqCst);
                }

                // Move to the following entry.
                match next {
                    None => break,
                    Some(e) => entry = e,
                }
            }
        }

        // Finally, search for the end of the skip list in order to unlink all the remaining
        // entries we have removed but not yet unlinked.
        self.search(None, guard);
    }
}

impl<K, V> Drop for SkipList<K, V> {
    fn drop(&mut self) {
        let mut node = self.head as *mut Node<K, V>;

        // Iterate through the whole skip list and destroy every node.
        while !node.is_null() {
            unsafe {
                // Unprotected loads are okay because this function is the only one currently using
                // the skip list.
                let next = (*node)
                    .tower(0)
                    .load(Ordering::Relaxed, epoch::unprotected());

                // Destroy the key and the value in non-head nodes only.
                if node as *const _ != self.head {
                    ptr::drop_in_place(&mut (*node).key);
                    ptr::drop_in_place(&mut (*node).value);
                }
                // Deallocate every node, even the head.
                Node::dealloc(node);

                node = next.as_raw() as *mut Node<K, V>;
            }
        }
    }
}

impl<K, V> IntoIterator for SkipList<K, V> {
    type Item = (K, V);
    type IntoIter = IntoIter<K, V>;

    fn into_iter(self) -> IntoIter<K, V> {
        unsafe {
            // Load the front node.
            //
            // Unprotected loads are okay because this function is the only one currently using
            // the skip list.
            let front = (*self.head)
                .tower(0)
                .load(Ordering::Relaxed, epoch::unprotected())
                .as_raw();

            // Clear the skip list by setting all pointers in head to null.
            for level in 0..MAX_HEIGHT {
                (*self.head)
                    .tower(level)
                    .store(Shared::null(), Ordering::Relaxed);
            }

            IntoIter {
                node: front as *mut Node<K, V>,
            }
        }
    }
}

/// A search result.
///
/// The result indicates whether the key was found, as well as what were the adjacent nodes to the
/// key on each level of the skip list.
struct Search<'g, K: 'g, V: 'g> {
    /// This flag is `true` if a node with the search key was found.
    ///
    /// More precisely, it will be `true` when `right[0].deref().key` equals the search key.
    found: bool,

    /// Adjacent nodes with smaller keys (predecessors).
    left: [&'g Node<K, V>; MAX_HEIGHT],

    /// Adjacent nodes with equal or greater keys (successors).
    right: [Shared<'g, Node<K, V>>; MAX_HEIGHT],
}

/// A reference-counted entry in a skip list.
pub struct Entry<'a, K: 'a, V: 'a> {
    pub parent: &'a SkipList<K, V>,
    pub node: &'a Node<K, V>,
}

unsafe impl<'a, K: Send + Sync, V: Send + Sync> Send for Entry<'a, K, V> {}
unsafe impl<'a, K: Send + Sync, V: Send + Sync> Sync for Entry<'a, K, V> {}

impl<'a, K, V> Entry<'a, K, V> {
    /// Constructs an entry from a raw pointer.
    unsafe fn from_raw(parent: &'a SkipList<K, V>, node: *const Node<K, V>) -> Self {
        Entry {
            parent,
            node: &*node,
        }
    }

    /// Returns `true` if the entry is removed from the skip list.
    pub fn is_removed(&self) -> bool {
        self.node.is_removed()
    }

    /// Returns a reference to the key.
    pub fn key(&self) -> &K {
        &self.node.key
    }

    /// Returns a reference to the value.
    pub fn value(&self) -> &V {
        &self.node.value
    }
}

impl<'a, K, V> Entry<'a, K, V>
where
    K: Ord,
{
    /// Moves to the next entry in the skip list.
    pub fn next(&mut self) -> bool {
        match self.get_next() {
            None => false,
            Some(n) => {
                *self = n;
                true
            }
        }
    }

    /// Moves to the previous entry in the skip list.
    pub fn prev(&mut self) -> bool {
        match self.get_prev() {
            None => false,
            Some(n) => {
                *self = n;
                true
            }
        }
    }

    /// Returns the next entry in the skip list.
    pub fn get_next(&self) -> Option<Entry<'a, K, V>> {
        let guard = &epoch::pin();

        loop {
            unsafe {
                // Load the successor to the current node.
                let mut succ = self.node.tower(0).load(Ordering::SeqCst, guard);

                if succ.tag() == 1 {
                    // If the pointer is marked, search for the current key.
                    let search = self.parent.search(Some(&self.node.key), guard);

                    if search.found {
                        // If an entry with the same key was found, load its successor.
                        succ = search.right[0]
                            .deref()
                            .tower(0)
                            .load(Ordering::SeqCst, guard);
                    } else {
                        // Otherwise, `right[0]` is the potential successor.
                        succ = search.right[0];
                    }

                    // If the newly loaded pointer is marked as well, go back to start.
                    if succ.tag() == 1 {
                        continue;
                    }
                };

                // Check if `succ` is a valid entry and, if so, return it.
                match succ.as_ref() {
                    None => return None,
                    Some(s) => {
                        if !s.is_removed() && Node::increment(s) {
                            return Some(Entry::from_raw(self.parent, s));
                        }
                    }
                }
            }
        }
    }

    /// Returns the previous entry in the skip list.
    pub fn get_prev(&self) -> Option<Entry<'a, K, V>> {
        let guard = &epoch::pin();

        loop {
            // Search for the current key  to get the probable predecessor to the current entry.
            let search = self.parent.search(Some(self.key()), guard);
            let pred = search.left[0];

            // If the predecessor is the head, there is no previous entry.
            if ptr::eq(pred, self.parent.head) {
                return None;
            }

            // Check if `pred` is a valid entry and, if so, return it.
            unsafe {
                if !pred.is_removed() && Node::increment(pred) {
                    return Some(Entry::from_raw(self.parent, pred));
                }
            }
        }
    }
}

impl<'a, K, V> Entry<'a, K, V>
where
    K: Ord + Send + 'static,
    V: Send + 'static,
{
    /// Removes the entry from the skip list.
    ///
    /// Returns `true` if this call removed the entry and `false` if it was already removed.
    pub fn remove(&self) -> bool {
        // Try marking the tower.
        if self.node.mark_tower() {
            // Success - the entry is removed. Now decrement `len`.
            self.parent.hot_data.len.fetch_sub(1, Ordering::SeqCst);

            // Search for the key to unlink the node from the skip list.
            self.parent.search(Some(&self.node.key), &epoch::pin());

            true
        } else {
            false
        }
    }
}

impl<'a, K, V> Drop for Entry<'a, K, V> {
    fn drop(&mut self) {
        unsafe { Node::decrement(self.node) }
    }
}

impl<'a, K, V> Clone for Entry<'a, K, V> {
    fn clone(&self) -> Entry<'a, K, V> {
        unsafe {
            // Incrementing will always succeed since we're already holding a reference to the node.
            Node::increment(self.node);
        }
        Entry {
            parent: self.parent,
            node: self.node,
        }
    }
}

/// An owning iterator over the entries of a `SkipList`.
pub struct IntoIter<K, V> {
    /// The current node.
    ///
    /// All preceeding nods have already been destroyed.
    node: *mut Node<K, V>,
}

impl<K, V> Drop for IntoIter<K, V> {
    fn drop(&mut self) {
        // Iterate through the whole chain and destroy every node.
        while !self.node.is_null() {
            unsafe {
                // Unprotected loads are okay because this function is the only one currently using
                // the skip list.
                let next = (*self.node)
                    .tower(0)
                    .load(Ordering::Relaxed, epoch::unprotected());

                ptr::drop_in_place(&mut (*self.node).key);
                ptr::drop_in_place(&mut (*self.node).value);
                Node::dealloc(self.node);

                self.node = next.as_raw() as *mut Node<K, V>;
            }
        }
    }
}

impl<K, V> Iterator for IntoIter<K, V> {
    type Item = (K, V);

    fn next(&mut self) -> Option<(K, V)> {
        loop {
            // Have we reached the end of the skip list?
            if self.node.is_null() {
                return None;
            }

            unsafe {
                // Take the key and value out of the node.
                let key = ptr::read(&mut (*self.node).key);
                let value = ptr::read(&mut (*self.node).value);

                // Get the next node in the skip list.
                //
                // Unprotected loads are okay because this function is the only one currently using
                // the skip list.
                let next = (*self.node)
                    .tower(0)
                    .load(Ordering::Relaxed, epoch::unprotected());

                // Deallocate the current node and move to the next one.
                Node::dealloc(self.node);
                self.node = next.as_raw() as *mut Node<K, V>;

                // The current node may be marked. If it is, it's been removed from the skip list
                // and we should just skip it.
                if next.tag() == 0 {
                    return Some((key, value));
                }
            }
        }
    }
}

/// An iterator over the entries of a `SkipList`.
pub struct Iter<'a, K: 'a, V: 'a> {
    /// The owning skip list.
    parent: &'a SkipList<K, V>,

    /// The last returned entry from the front.
    front: Option<Entry<'a, K, V>>,

    /// The last returned entry from the back.
    back: Option<Entry<'a, K, V>>,

    /// `true` if iteration has finished (the front and back have met).
    finished: bool,
}

impl<'a, K, V> Iterator for Iter<'a, K, V>
where
    K: Ord,
{
    type Item = Entry<'a, K, V>;

    fn next(&mut self) -> Option<Entry<'a, K, V>> {
        if self.finished {
            None
        } else {
            // Advance the front one step forward.
            if self.front.is_none() {
                self.front = self.parent.front();
            } else {
                if !self.front.as_mut().unwrap().next() {
                    self.finished = true;
                    return None;
                }
            }

            // Check whether the front and the back have met.
            if let Some(f) = self.front.as_ref() {
                if let Some(b) = self.back.as_ref() {
                    if f.key() >= b.key() {
                        self.finished = true;
                        return None;
                    }
                }
                Some(f.clone())
            } else {
                self.finished = true;
                None
            }
        }
    }
}

impl<'a, K, V> DoubleEndedIterator for Iter<'a, K, V>
where
    K: Ord,
{
    fn next_back(&mut self) -> Option<Entry<'a, K, V>> {
        if self.finished {
            None
        } else {
            // Advance the back one step backward.
            if self.back.is_none() {
                self.back = self.parent.back();
            } else {
                if !self.back.as_mut().unwrap().prev() {
                    self.finished = true;
                    return None;
                }
            }

            // Check whether the front and the back have met.
            if let Some(b) = self.back.as_ref() {
                if let Some(f) = self.front.as_ref() {
                    if f.key() >= b.key() {
                        self.finished = true;
                        return None;
                    }
                }
                Some(b.clone())
            } else {
                self.finished = true;
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, ATOMIC_USIZE_INIT, Ordering};

    use super::SkipList;

    #[test]
    fn new() {
        SkipList::<i32, i32>::new();
        SkipList::<String, Box<i32>>::new();
    }

    #[test]
    fn is_empty() {
        let s = SkipList::new();
        assert!(s.is_empty());

        s.insert(1, 10);
        assert!(!s.is_empty());
        s.insert(2, 20);
        s.insert(3, 30);
        assert!(!s.is_empty());

        s.remove(&2);
        assert!(!s.is_empty());

        s.remove(&1);
        assert!(!s.is_empty());

        s.remove(&3);
        assert!(s.is_empty());
    }

    #[test]
    fn insert() {
        let insert = [0, 4, 2, 12, 8, 7, 11, 5];
        let not_present = [1, 3, 6, 9, 10];
        let s = SkipList::new();

        for &x in &insert {
            s.insert(x, x * 10);
            assert_eq!(*s.get(&x).unwrap().value(), x * 10);
        }

        for &x in &not_present {
            assert!(s.get(&x).is_none());
        }
    }

    #[test]
    fn remove() {
        let insert = [0, 4, 2, 12, 8, 7, 11, 5];
        let not_present = [1, 3, 6, 9, 10];
        let remove = [2, 12, 8];
        let remaining = [0, 4, 5, 7, 11];

        let s = SkipList::new();

        for &x in &insert {
            s.insert(x, x * 10);
        }
        for x in &not_present {
            assert!(s.remove(x).is_none());
        }
        for x in &remove {
            assert!(s.remove(x).is_some());
        }

        let mut v = vec![];
        let mut e = s.front().unwrap();
        loop {
            v.push(*e.key());
            if !e.next() {
                break;
            }
        }

        assert_eq!(v, remaining);
        for x in &insert {
            s.remove(x);
        }
        assert!(s.is_empty());
    }

    #[test]
    fn entry() {
        let s = SkipList::new();

        assert!(s.front().is_none());
        assert!(s.back().is_none());

        for &x in &[4, 2, 12, 8, 7, 11, 5] {
            s.insert(x, x * 10);
        }

        let mut e = s.front().unwrap();
        assert_eq!(*e.key(), 2);
        assert!(!e.prev());
        assert!(e.next());
        assert_eq!(*e.key(), 4);

        e = s.back().unwrap();
        assert_eq!(*e.key(), 12);
        assert!(!e.next());
        assert!(e.prev());
        assert_eq!(*e.key(), 11);
    }

    #[test]
    fn entry_remove() {
        let s = SkipList::new();

        for &x in &[4, 2, 12, 8, 7, 11, 5] {
            s.insert(x, x * 10);
        }

        let mut e = s.get(&7).unwrap();
        assert!(!e.is_removed());
        assert!(e.remove());
        assert!(e.is_removed());

        e.prev();
        e.next();
        assert_ne!(*e.key(), 7);

        for e in s.iter() {
            assert!(!s.is_empty());
            assert_ne!(s.len(), 0);
            e.remove();
        }
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn entry_reposition() {
        let s = SkipList::new();

        for &x in &[4, 2, 12, 8, 7, 11, 5] {
            s.insert(x, x * 10);
        }

        let mut e = s.get(&7).unwrap();
        assert!(!e.is_removed());
        assert!(e.remove());
        assert!(e.is_removed());

        s.insert(7, 700);
        e.prev();
        e.next();
        assert_eq!(*e.key(), 7);
    }

    #[test]
    fn len() {
        let s = SkipList::new();
        assert_eq!(s.len(), 0);

        for (i, &x) in [4, 2, 12, 8, 7, 11, 5].iter().enumerate() {
            s.insert(x, x * 10);
            assert_eq!(s.len(), i + 1);
        }

        s.insert(5, 0);
        assert_eq!(s.len(), 7);
        s.insert(5, 0);
        assert_eq!(s.len(), 7);

        s.remove(&6);
        assert_eq!(s.len(), 7);
        s.remove(&5);
        assert_eq!(s.len(), 6);
        s.remove(&12);
        assert_eq!(s.len(), 5);
    }

    #[test]
    fn insert_and_remove() {
        let s = SkipList::new();
        let keys = || s.iter().map(|e| *e.key()).collect::<Vec<_>>();

        s.insert(3, 0);
        s.insert(5, 0);
        s.insert(1, 0);
        s.insert(4, 0);
        s.insert(2, 0);
        assert_eq!(keys(), [1, 2, 3, 4, 5]);

        assert!(s.remove(&4).is_some());
        assert_eq!(keys(), [1, 2, 3, 5]);
        assert!(s.remove(&3).is_some());
        assert_eq!(keys(), [1, 2, 5]);
        assert!(s.remove(&1).is_some());
        assert_eq!(keys(), [2, 5]);

        assert!(s.remove(&1).is_none());
        assert_eq!(keys(), [2, 5]);
        assert!(s.remove(&3).is_none());
        assert_eq!(keys(), [2, 5]);

        assert!(s.remove(&2).is_some());
        assert_eq!(keys(), [5]);
        assert!(s.remove(&5).is_some());
        assert_eq!(keys(), []);

        s.insert(3, 0);
        assert_eq!(keys(), [3]);
        s.insert(1, 0);
        assert_eq!(keys(), [1, 3]);
        s.insert(3, 0);
        assert_eq!(keys(), [1, 3]);
        s.insert(5, 0);
        assert_eq!(keys(), [1, 3, 5]);

        assert!(s.remove(&3).is_some());
        assert_eq!(keys(), [1, 5]);
        assert!(s.remove(&1).is_some());
        assert_eq!(keys(), [5]);
        assert!(s.remove(&3).is_none());
        assert_eq!(keys(), [5]);
        assert!(s.remove(&5).is_some());
        assert_eq!(keys(), []);
    }

    #[test]
    fn get_and_seek() {
        let s = SkipList::new();
        s.insert(30, 3);
        s.insert(50, 5);
        s.insert(10, 1);
        s.insert(40, 4);
        s.insert(20, 2);

        assert_eq!(*s.get(&10).unwrap().value(), 1);
        assert_eq!(*s.get(&20).unwrap().value(), 2);
        assert_eq!(*s.get(&30).unwrap().value(), 3);
        assert_eq!(*s.get(&40).unwrap().value(), 4);
        assert_eq!(*s.get(&50).unwrap().value(), 5);

        assert!(s.get(&7).is_none());
        assert!(s.get(&27).is_none());
        assert!(s.get(&31).is_none());
        assert!(s.get(&97).is_none());

        assert_eq!(*s.seek(&10).unwrap().value(), 1);
        assert_eq!(*s.seek(&20).unwrap().value(), 2);
        assert_eq!(*s.seek(&30).unwrap().value(), 3);
        assert_eq!(*s.seek(&40).unwrap().value(), 4);
        assert_eq!(*s.seek(&50).unwrap().value(), 5);

        assert_eq!(*s.seek(&7).unwrap().value(), 1);
        assert_eq!(*s.seek(&27).unwrap().value(), 3);
        assert_eq!(*s.seek(&31).unwrap().value(), 4);
        assert!(s.get(&97).is_none());
    }

    #[test]
    fn get_or_insert() {
        let s = SkipList::new();
        s.insert(3, 3);
        s.insert(5, 5);
        s.insert(1, 1);
        s.insert(4, 4);
        s.insert(2, 2);

        assert_eq!(*s.get(&4).unwrap().value(), 4);
        assert_eq!(*s.insert(4, 40).value(), 40);
        assert_eq!(*s.get(&4).unwrap().value(), 40);

        assert_eq!(*s.get_or_insert(4, 400).value(), 40);
        assert_eq!(*s.get(&4).unwrap().value(), 40);
        assert_eq!(*s.get_or_insert(6, 600).value(), 600);
    }

    #[test]
    fn get_next_prev() {
        let s = SkipList::new();
        s.insert(3, 3);
        s.insert(5, 5);
        s.insert(1, 1);
        s.insert(4, 4);
        s.insert(2, 2);

        let mut e = s.get(&3).unwrap();
        assert_eq!(*e.get_next().unwrap().value(), 4);
        assert_eq!(*e.get_prev().unwrap().value(), 2);
        assert_eq!(*e.value(), 3);

        e.prev();
        assert_eq!(*e.get_next().unwrap().value(), 3);
        assert_eq!(*e.get_prev().unwrap().value(), 1);
        assert_eq!(*e.value(), 2);

        e.prev();
        assert_eq!(*e.get_next().unwrap().value(), 2);
        assert!(e.get_prev().is_none());
        assert_eq!(*e.value(), 1);

        e.next();
        e.next();
        e.next();
        e.next();
        assert!(e.get_next().is_none());
        assert_eq!(*e.get_prev().unwrap().value(), 4);
        assert_eq!(*e.value(), 5);
    }

    #[test]
    fn front_and_back() {
        let s = SkipList::new();
        assert!(s.front().is_none());
        assert!(s.back().is_none());

        for &x in &[4, 2, 12, 8, 7, 11, 5] {
            s.insert(x, x * 10);
        }

        assert_eq!(*s.front().unwrap().key(), 2);
        assert_eq!(*s.back().unwrap().key(), 12);
    }

    #[test]
    fn iter() {
        let s = SkipList::new();
        for &x in &[4, 2, 12, 8, 7, 11, 5] {
            s.insert(x, x * 10);
        }

        assert_eq!(
            s.iter().map(|e| *e.key()).collect::<Vec<_>>(),
            &[2, 4, 5, 7, 8, 11, 12]
        );
        assert_eq!(
            s.iter().rev().map(|e| *e.key()).collect::<Vec<_>>(),
            &[12, 11, 8, 7, 5, 4, 2]
        );

        let mut it = s.iter();
        s.remove(&2);
        assert_eq!(*it.next().unwrap().key(), 4);
        assert_eq!(*it.next_back().unwrap().key(), 12);
        s.remove(&7);
        assert_eq!(*it.next().unwrap().key(), 5);
        s.remove(&5);
        assert_eq!(*it.next().unwrap().key(), 8);
        assert_eq!(*it.next_back().unwrap().key(), 11);

        assert!(it.next_back().is_none());
        assert!(it.next().is_none());
    }

    #[test]
    fn into_iter() {
        let s = SkipList::new();
        for &x in &[4, 2, 12, 8, 7, 11, 5] {
            s.insert(x, x * 10);
        }

        assert_eq!(
            s.into_iter().collect::<Vec<_>>(),
            &[
                (2, 20),
                (4, 40),
                (5, 50),
                (7, 70),
                (8, 80),
                (11, 110),
                (12, 120)
            ]
        );
    }

    #[test]
    fn clear() {
        let s = SkipList::new();
        for &x in &[4, 2, 12, 8, 7, 11, 5] {
            s.insert(x, x * 10);
        }

        assert!(!s.is_empty());
        assert_ne!(s.len(), 0);
        s.clear();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn drops() {
        static KEYS: AtomicUsize = ATOMIC_USIZE_INIT;
        static VALUES: AtomicUsize = ATOMIC_USIZE_INIT;

        #[derive(Eq, PartialEq, Ord, PartialOrd)]
        struct Key(i32);

        impl Drop for Key {
            fn drop(&mut self) {
                KEYS.fetch_add(1, Ordering::SeqCst);
            }
        }

        struct Value;

        impl Drop for Value {
            fn drop(&mut self) {
                VALUES.fetch_add(1, Ordering::SeqCst);
            }
        }

        let s = SkipList::new();
        for &x in &[4, 2, 12, 8, 7, 11, 5] {
            s.insert(Key(x), Value);
        }
        assert_eq!(KEYS.load(Ordering::SeqCst), 0);
        assert_eq!(VALUES.load(Ordering::SeqCst), 0);

        let key7 = Key(7);
        s.remove(&key7);
        assert_eq!(KEYS.load(Ordering::SeqCst), 0);
        assert_eq!(VALUES.load(Ordering::SeqCst), 0);

        drop(s);

        // TODO(stjepang): When we get per-SkipList collectors, this for loop will become
        // unnecessary.
        for _ in 0..1000 {
            ::epoch::pin().flush();
        }

        assert_eq!(KEYS.load(Ordering::SeqCst), 7);
        assert_eq!(VALUES.load(Ordering::SeqCst), 7);
    }
}
