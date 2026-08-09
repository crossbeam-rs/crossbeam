use crate::{alloc_helper::Global, seg_queue::BLOCK_CAP};
use alloc::{alloc::handle_alloc_error, boxed::Box};
use core::{
    alloc::Layout,
    cell::UnsafeCell,
    marker::PhantomData,
    mem::{MaybeUninit, forget},
    ptr::{NonNull, drop_in_place, null_mut, slice_from_raw_parts_mut},
    sync::atomic::{
        AtomicPtr, AtomicU16, AtomicU64, AtomicUsize,
        Ordering::{self, AcqRel, Acquire, Relaxed, SeqCst},
        fence,
    },
};
use crossbeam_utils::{Backoff, CachePadded};

fn expect_u16(size: usize) -> u16 {
    u16::try_from(size).expect("block size is <= u16:::MAX")
}

const ZERO: usize = 0;
const WRITTEN: usize = 1;

struct Slot<T> {
    value: UnsafeCell<MaybeUninit<T>>,
    state: AtomicUsize,
}

impl<T> Slot<T> {
    fn wait(&self) -> T {
        let backoff = Backoff::new();
        while self.state.load(Ordering::Acquire) & WRITTEN == 0 {
            backoff.snooze();
        }

        unsafe { self.value.get().read().assume_init() }
    }

    fn write(&self, val: T) {
        unsafe { self.value.get().write(MaybeUninit::new(val)) };
        self.state.store(WRITTEN, Ordering::Release);
    }
}

#[repr(C)]
struct Block<T> {
    next: AtomicPtr<Self>,
    consumed: AtomicUsize,
    len: usize,

    _slots: PhantomData<Slot<T>>,
}

impl<T> Block<T> {
    fn layout(len: usize) -> Option<(Layout, usize)> {
        let slots = Layout::array::<Slot<T>>(len).ok()?;
        let (layout, slots_offset) = Layout::new::<Self>().extend(slots).ok()?;

        Some((layout.pad_to_align(), slots_offset))
    }

    fn new(len: usize) -> NonNull<Self> {
        let (layout, slots_offset) = Self::layout(len).expect("block layout overflow");

        let allocation = Global
            .allocate(layout)
            .unwrap_or_else(|| handle_alloc_error(layout));

        let block = allocation.cast::<Self>();

        unsafe {
            block.as_ptr().write(Self {
                next: AtomicPtr::new(null_mut()),
                consumed: AtomicUsize::new(0),
                len,
                _slots: PhantomData,
            });

            let slots = allocation.as_ptr().add(slots_offset).cast::<Slot<T>>();

            for i in 0..len {
                slots.add(i).write(MaybeUninit::zeroed().assume_init());
            }
        }

        block
    }

    unsafe fn free(this: *mut Self) {
        unsafe {
            let len = (*this).len;

            let (layout, slots_offset) = Self::layout(len).expect("valid stored block length");
            let slots = this.cast::<u8>().add(slots_offset).cast::<Slot<T>>();

            drop_in_place(slice_from_raw_parts_mut(slots, len));
            drop_in_place(this);

            Global.deallocate(NonNull::new_unchecked(this.cast()), layout);
        }
    }

    fn wait_next(&self) -> *mut Self {
        let backoff = Backoff::new();

        loop {
            let next = self.next.load(Ordering::Acquire);
            if !next.is_null() {
                return next;
            }
            backoff.snooze();
        }
    }

    unsafe fn slot_ptr(this: *mut Self, index: usize) -> *mut Slot<T> {
        let len = unsafe { (*this).len };
        debug_assert!(index < len);

        let (_, slots_offset) = Self::layout(len).expect("valid stored block length");

        unsafe {
            this.cast::<u8>()
                .add(slots_offset)
                .cast::<Slot<T>>()
                .add(index)
        }
    }
}

struct BlockChain<T> {
    root: *mut Block<T>,
    size: usize,
}

impl<T> BlockChain<T> {
    fn new() -> Self {
        Self {
            root: null_mut(),
            size: 0,
        }
    }

    fn grow_to_fit(&mut self, len: usize) {
        while self.size <= len {
            self.add_block(Block::new(
                BLOCK_CAP.max(u16::try_from(len + 1 - self.size).unwrap_or(u16::MAX) as usize),
            ));
        }
    }

    /// Note: Always resets `block`, e.g., loses it's tail, if it has one.
    fn add_block(&mut self, mut block: NonNull<Block<T>>) {
        unsafe {
            *block.as_mut().next.get_mut() = self.root;
            self.size += block.as_ref().len;
            self.root = block.as_ptr();
        }
    }

    /// Does not clear size.
    fn take_head(&mut self) -> *mut Block<T> {
        let Self { root, .. } = *self;

        self.root = null_mut();

        root
    }
}

impl<T> Drop for BlockChain<T> {
    fn drop(&mut self) {
        let mut block = self.root;

        loop {
            if block.is_null() {
                break;
            }

            let ptr = block;
            block = unsafe { &*block }.next.load(Relaxed);

            unsafe { Block::free(ptr) };
        }
    }
}

struct Position<T> {
    index: AtomicU64,
    block: AtomicPtr<Block<T>>,
}

struct PHead {
    token: u32,
    block_length: u16,
    index: u16,
}

impl PHead {
    fn to_u64(&self) -> u64 {
        let Self {
            token,
            block_length,
            index,
        } = *self;

        (u64::from(token) << 32) | (u64::from(block_length) << 16) | u64::from(index)
    }

    fn is_zero(&self) -> bool {
        self.to_u64() == 0
    }

    fn from_u64(value: u64) -> Self {
        Self {
            token: (value >> 32) as u32,
            block_length: (value >> 16) as u16,
            index: value as u16,
        }
    }
}

#[derive(PartialEq)]
struct CHead {
    has_next: bool,
    token: u32, // real size: u31 (top bit overridden by has_next)
    block_length: u16,
    index: u16,
}

const TOKEN_MASK: u32 = 0x7fff_ffff;

impl CHead {
    // Explicitly lists fields on purpose, equivalent to Self::from_u64(0).
    const ZERO: Self = Self {
        has_next: false,
        token: 0,
        block_length: 0,
        index: 0,
    };

    fn to_u64(&self) -> u64 {
        (u64::from(self.has_next) << 63)
            | (u64::from(self.token & TOKEN_MASK) << 32)
            | (u64::from(self.block_length) << 16)
            | u64::from(self.index)
    }

    fn from_u64(value: u64) -> Self {
        Self {
            has_next: value >> 63 != 0,
            token: ((value >> 32) as u32) & TOKEN_MASK,
            block_length: (value >> 16) as u16,
            index: value as u16,
        }
    }

    fn inc_token(&self) -> u32 {
        self.token.wrapping_add(1) & TOKEN_MASK
    }
}

pub struct BatchQueue<T> {
    phead: CachePadded<Position<T>>,
    chead: CachePadded<Position<T>>,

    _marker: PhantomData<T>,
}

impl<T> Drop for BatchQueue<T> {
    fn drop(&mut self) {
        let (phead, pblock) = self.acquire_pinfo_mut();
        let (chead, cblock) = self.acquire_cinfo_mut();
        let mut current = cblock;

        while let Some(cur) = unsafe { current.as_mut() } {
            let mut upper = expect_u16(cur.len);

            if pblock == current {
                upper = upper.min(phead.index);
            }

            let mut lower = 0;

            if cblock == current {
                lower = lower.max(chead.index);
            }

            let owned = lower..upper;
            let owned_len = owned.len();

            for i in owned {
                unsafe {
                    (&mut *Block::slot_ptr(current, usize::from(i)))
                        .value
                        .get_mut()
                        .assume_init_drop()
                };
            }

            let next = *cur.next.get_mut();

            let supper = usize::from(upper);

            if owned_len >= supper
                || cur
                    .consumed
                    .fetch_add(owned_len + (cur.len - supper), AcqRel)
                    + owned_len
                    == supper
            {
                unsafe { Block::free(current) };
            }

            current = next;
        }
    }
}

impl<T> BatchQueue<T> {
    pub const fn new() -> Self {
        Self {
            phead: CachePadded::new(Position {
                index: AtomicU64::new(0),
                block: AtomicPtr::new(null_mut()),
            }),
            chead: CachePadded::new(Position {
                index: AtomicU64::new(0),
                block: AtomicPtr::new(null_mut()),
            }),

            _marker: PhantomData,
        }
    }

    fn acquire_pinfo_mut(&mut self) -> (PHead, *mut Block<T>) {
        (
            PHead::from_u64(*self.phead.index.get_mut()),
            *self.phead.block.get_mut(),
        )
    }

    fn acquire_pinfo(&self) -> (PHead, *mut Block<T>) {
        (
            PHead::from_u64(self.phead.index.load(Ordering::Acquire)),
            self.acquire_pblock(),
        )
    }

    fn acquire_pblock(&self) -> *mut Block<T> {
        self.phead.block.load(Ordering::Acquire)
    }

    fn store_pinfo(&self, phead: PHead, block: *mut Block<T>) {
        self.phead.block.store(block, Ordering::Release);
        self.phead.index.store(phead.to_u64(), Ordering::Release);
    }

    pub fn push<I>(&self, items: I)
    where
        I: IntoIterator<Item = T>,
        I::IntoIter: ExactSizeIterator,
    {
        let mut items = items.into_iter();
        let len = items.len();

        if len == 0 {
            return;
        }

        let backoff = Backoff::new();
        let (mut head, mut block) = self.acquire_pinfo();

        let mut block_chain = BlockChain::new();

        loop {
            let mut owns_initial_reservation = false;

            if head.index >= head.block_length && !block.is_null() {
                backoff.snooze();
                (head, block) = self.acquire_pinfo();
                continue;
            }

            let cap = usize::from(head.block_length - head.index);

            if cap <= len {
                block_chain.grow_to_fit(len - cap);
            }

            if block.is_null() {
                debug_assert!(block_chain.size != 0);

                let new = block_chain.take_head();

                match self.phead.block.compare_exchange(
                    null_mut(),
                    new,
                    Ordering::Release,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        block = new;

                        let blen = expect_u16(unsafe { (&*block).len });

                        self.store_cinfo(
                            CHead {
                                block_length: blen,
                                ..CHead::ZERO
                            },
                            block,
                        );

                        head.block_length = blen;
                        // A non-null block with a zero head is a transitional state, so
                        // competing producers wait until this first reservation is published.
                        owns_initial_reservation = true;
                    }
                    Err(_) => {
                        block_chain.root = new;
                        (head, block) = self.acquire_pinfo();
                        continue;
                    }
                }
            }

            let new_head = PHead {
                index: head
                    .index
                    .saturating_add(u16::try_from(len).unwrap_or(u16::MAX)),

                ..head
            };

            let reservation = if owns_initial_reservation {
                Ok(())
            } else {
                self.phead
                    .index
                    .compare_exchange_weak(
                        head.to_u64(),
                        new_head.to_u64(),
                        Ordering::SeqCst,
                        Ordering::Acquire,
                    )
                    .map(|_| ())
            };

            match reservation {
                Err(real) => {
                    head = PHead::from_u64(real);
                    block = self.acquire_pblock();
                    backoff.spin();
                }
                Ok(()) => unsafe {
                    if new_head.index >= new_head.block_length {
                        let mut filled = usize::from(head.block_length - head.index);
                        let mut current = block;

                        let mut info = None;

                        let mut num_blocks: usize = 0;

                        while let Some(next) =
                            NonNull::new(current.as_ref_unchecked().next.load(Ordering::Relaxed))
                                .or_else(|| {
                                    let block_chain_head = NonNull::new(block_chain.take_head())?;

                                    current
                                        .as_ref_unchecked()
                                        .next
                                        .store(block_chain_head.as_ptr(), Ordering::Release);

                                    Some(block_chain_head)
                                })
                        {
                            let next_len = next.as_ref().len;

                            // Inside of this `next` is where we terminate our reservation.
                            if filled <= len && len - filled < next_len {
                                info = Some((expect_u16(len - filled), next, expect_u16(next_len)));
                            }

                            if filled <= len {
                                num_blocks += 1;
                            }

                            filled += next_len;
                            current = next.as_ptr();
                        }

                        let (index, block, block_length) =
                            info.expect("should have room for `len`");

                        let new_phead = PHead {
                            token: head.token.wrapping_add(num_blocks as u32),
                            block_length,
                            index,
                        };

                        self.phead.block.store(block.as_ptr(), Ordering::Release);
                        self.phead
                            .index
                            .store(new_phead.to_u64(), Ordering::Release);
                    } else if owns_initial_reservation {
                        self.phead.index.store(new_head.to_u64(), Ordering::Release);
                    }

                    let mut current_block = &*block;
                    let mut current_index = head.index;

                    // TODO: Can we unwrap this at all?
                    for _ in 0..len {
                        // Q: Should we handle iterator panics?
                        // A: No. An ExactSizeIterator producing less than
                        // `len` items is a catastrophic failure, and so is any
                        // panic in iter::next. We choose to propogate this
                        // catastrophic error.
                        let item = items.next().expect("ExactSizeIterator has `len` elements");
                        let current_block_ptr = current_block as *const _ as *mut _;

                        let slot = &*Block::slot_ptr(current_block_ptr, current_index as usize);

                        current_index += 1;
                        if current_index >= expect_u16(current_block.len) {
                            current_block = &*current_block.next.load(Ordering::Relaxed);
                            current_index = 0;
                        }

                        slot.write(item);
                    }

                    return;
                },
            }
        }
    }

    fn acquire_cinfo_mut(&mut self) -> (CHead, *mut Block<T>) {
        (
            CHead::from_u64(*self.chead.index.get_mut()),
            *self.chead.block.get_mut(),
        )
    }

    fn acquire_cinfo(&self) -> (CHead, *mut Block<T>) {
        (
            CHead::from_u64(self.chead.index.load(Ordering::Acquire)),
            self.acquire_cblock(),
        )
    }

    fn acquire_cblock(&self) -> *mut Block<T> {
        self.chead.block.load(Ordering::Acquire)
    }

    fn store_cinfo(&self, chead: CHead, block: *mut Block<T>) {
        self.chead.block.store(block, Ordering::Release);
        self.chead.index.store(chead.to_u64(), Ordering::Release);
    }

    pub fn pop(&self, req: usize) -> BQIter<T> {
        let backoff = Backoff::new();
        let (mut head, mut block) = self.acquire_cinfo();
        let mut len;

        loop {
            if head == CHead::ZERO {
                return BQIter::EMPTY;
            }

            if head.index >= head.block_length {
                backoff.snooze();
                (head, block) = self.acquire_cinfo();
                continue;
            }

            let mut new_head = CHead {
                index: head
                    .index
                    .saturating_add(u16::try_from(req).unwrap_or(u16::MAX)),
                ..head
            };

            if !head.has_next {
                fence(SeqCst);
                let (phead, pblock) = self.acquire_pinfo();

                new_head.has_next = pblock != block || head.token != (phead.token & TOKEN_MASK);

                if !new_head.has_next {
                    if head.index == phead.index {
                        return BQIter::EMPTY;
                    }

                    new_head.index = phead.index.min(new_head.index);
                }
            }

            match self.chead.index.compare_exchange_weak(
                head.to_u64(),
                new_head.to_u64(),
                Ordering::Release,
                Ordering::Acquire,
            ) {
                Ok(_) => unsafe {
                    len = usize::from(new_head.index.min(new_head.block_length) - head.index);

                    if new_head.index >= new_head.block_length {
                        let get_next = |current: *mut Block<T>| {
                            loop {
                                if let Some(next) = NonNull::new((&*current).next.load(Acquire)) {
                                    break next.as_ptr();
                                }

                                backoff.snooze();
                            }
                        };

                        let (mut phead, mut pblock) = loop {
                            let (phead, pblock) = self.acquire_pinfo();

                            if pblock != block && (phead.token & TOKEN_MASK) != head.token {
                                break (phead, pblock);
                            }

                            backoff.snooze();
                        };

                        let mut new_block = get_next(block);
                        let mut new_head = CHead {
                            token: head.token,
                            ..CHead::ZERO
                        };

                        // We would like to reserve into subsequent blocks
                        // until we reach a terminated phead or fill our request.

                        loop {
                            let blen = (&*new_block).len;
                            new_head.token = new_head.inc_token();

                            new_head = CHead {
                                block_length: expect_u16(blen),
                                index: (len + blen > req)
                                    .then(|| expect_u16(req - len))
                                    .unwrap_or(u16::MAX),
                                ..new_head
                            };

                            if pblock == new_block && new_head.index >= new_head.block_length {
                                if phead.index >= phead.block_length {
                                    loop {
                                        backoff.snooze();

                                        let (new_phead, new_pblock) = self.acquire_pinfo();

                                        if (new_pblock != pblock && new_phead.token != phead.token)
                                            || new_phead.index < new_phead.block_length
                                        {
                                            (phead, pblock) = (new_phead, new_pblock);
                                            break;
                                        }
                                    }
                                }
                            }

                            if new_head.index < new_head.block_length || pblock == new_block {
                                if pblock == new_block {
                                    new_head.index = phead.index.min(new_head.index);
                                }

                                len += usize::from(new_head.index);
                                break;
                            }

                            len += blen;

                            new_block = get_next(new_block);
                        }

                        self.store_cinfo(new_head, new_block)
                    }

                    break BQIter {
                        block,
                        index: usize::from(head.index),
                        len,
                    };
                },

                Err(real) => {
                    head = CHead::from_u64(real);
                    block = self.chead.block.load(Acquire);
                    backoff.spin();
                }
            }
        }
    }
}

pub struct BQIter<T> {
    block: *mut Block<T>,
    index: usize,
    len: usize,
}

impl<T> BQIter<T> {
    const EMPTY: Self = Self {
        block: null_mut(),
        index: 0,
        len: 0,
    };
}

impl<T> ExactSizeIterator for BQIter<T> {}

impl<T> Iterator for BQIter<T> {
    type Item = T;

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len, Some(self.len))
    }

    fn next(&mut self) -> Option<Self::Item> {
        if self.len == 0 {
            return None;
        }

        unsafe {
            let slot = Block::slot_ptr(self.block, self.index);
            let block_ptr = self.block;
            let block = &*block_ptr;

            self.index += 1;
            self.len -= 1;

            if self.len != 0 && self.index >= block.len {
                self.block = block.next.load(Acquire);
                self.index = 0;
            }

            let item = (&*slot).wait();

            if block.consumed.fetch_add(1, AcqRel) + 1 == usize::from(block.len) {
                Block::free(block_ptr)
            }

            Some(item)
        }
    }
}

impl<T> Drop for BQIter<T> {
    fn drop(&mut self) {
        while let Some(_drop) = self.next() {}
    }
}
