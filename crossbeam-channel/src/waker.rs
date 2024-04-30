//! Waking mechanism for threads blocked on channel operations.

use core::sync::atomic::AtomicU32;
use std::ptr;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::thread::{self, ThreadId};
use std::vec::Vec;

use crate::context::Context;
use crate::select::{Operation, Selected};

/// Represents a thread blocked on a specific channel operation.
pub(crate) struct Entry {
    /// The operation.
    pub(crate) oper: Operation,

    /// Optional packet.
    pub(crate) packet: *mut (),

    /// Context associated with the thread owning this operation.
    pub(crate) cx: Context,
}

/// A queue of threads blocked on channel operations.
///
/// This data structure is used by threads to register blocking operations and get woken up once
/// an operation becomes ready.
pub(crate) struct Waker {
    /// A list of select operations.
    selectors: Vec<Entry>,

    /// A list of operations waiting to be ready.
    observers: Vec<Entry>,
}

impl Waker {
    /// Creates a new `Waker`.
    #[inline]
    pub(crate) fn new() -> Self {
        Self {
            selectors: Vec::new(),
            observers: Vec::new(),
        }
    }

    /// Registers a select operation.
    #[inline]
    pub(crate) fn register(&mut self, oper: Operation, cx: &Context) {
        self.register_with_packet(oper, ptr::null_mut(), cx);
    }

    /// Registers a select operation and a packet.
    #[inline]
    pub(crate) fn register_with_packet(&mut self, oper: Operation, packet: *mut (), cx: &Context) {
        self.selectors.push(Entry {
            oper,
            packet,
            cx: cx.clone(),
        });
    }

    /// Unregisters a select operation.
    #[inline]
    pub(crate) fn unregister(&mut self, oper: Operation) -> Option<Entry> {
        if let Some((i, _)) = self
            .selectors
            .iter()
            .enumerate()
            .find(|&(_, entry)| entry.oper == oper)
        {
            let entry = self.selectors.remove(i);
            Some(entry)
        } else {
            None
        }
    }

    /// Attempts to find another thread's entry, select the operation, and wake it up.
    #[inline]
    pub(crate) fn try_select(&mut self) -> Option<Entry> {
        if self.selectors.is_empty() {
            None
        } else {
            let thread_id = current_thread_id();

            self.selectors
                .iter()
                .position(|selector| {
                    // Does the entry belong to a different thread?
                    selector.cx.thread_id() != thread_id
                        && selector // Try selecting this operation.
                            .cx
                            .try_select(Selected::Operation(selector.oper))
                            .is_ok()
                        && {
                            // Provide the packet.
                            selector.cx.store_packet(selector.packet);
                            // Wake the thread up.
                            selector.cx.unpark();
                            true
                        }
                })
                // Remove the entry from the queue to keep it clean and improve
                // performance.
                .map(|pos| self.selectors.remove(pos))
        }
    }

    /// Returns `true` if there is an entry which can be selected by the current thread.
    #[inline]
    pub(crate) fn can_select(&self) -> bool {
        if self.selectors.is_empty() {
            false
        } else {
            let thread_id = current_thread_id();

            self.selectors.iter().any(|entry| {
                entry.cx.thread_id() != thread_id && entry.cx.selected() == Selected::Waiting
            })
        }
    }

    /// Registers an operation waiting to be ready.
    #[inline]
    pub(crate) fn watch(&mut self, oper: Operation, cx: &Context) {
        self.observers.push(Entry {
            oper,
            packet: ptr::null_mut(),
            cx: cx.clone(),
        });
    }

    /// Unregisters an operation waiting to be ready.
    #[inline]
    pub(crate) fn unwatch(&mut self, oper: Operation) {
        self.observers.retain(|e| e.oper != oper);
    }

    /// Notifies all operations waiting to be ready.
    #[inline]
    pub(crate) fn notify(&mut self) {
        for entry in self.observers.drain(..) {
            if entry.cx.try_select(Selected::Operation(entry.oper)).is_ok() {
                entry.cx.unpark();
            }
        }
    }

    /// Notifies all registered operations that the channel is disconnected.
    #[inline]
    pub(crate) fn disconnect(&mut self) {
        for entry in self.selectors.iter() {
            if entry.cx.try_select(Selected::Disconnected).is_ok() {
                // Wake the thread up.
                //
                // Here we don't remove the entry from the queue. Registered threads must
                // unregister from the waker by themselves. They might also want to recover the
                // packet value and destroy it, if necessary.
                entry.cx.unpark();
            }
        }

        self.notify();
    }
}

impl Drop for Waker {
    #[inline]
    fn drop(&mut self) {
        debug_assert_eq!(self.selectors.len(), 0);
        debug_assert_eq!(self.observers.len(), 0);
    }
}

/// A waker that can be shared among threads without locking.
///
/// This is a simple wrapper around `Waker` that internally uses a mutex for synchronization.
pub(crate) struct SyncWaker {
    /// The inner `Waker`.
    inner: Mutex<Waker>,

    /// Atomic state for this waker.
    state: WakerState,
}

impl SyncWaker {
    /// Creates a new `SyncWaker`.
    #[inline]
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(Waker::new()),
            state: WakerState::new(),
        }
    }

    /// Returns a token that can be used to manage the state of a blocking operation.
    pub(crate) fn start(&self) -> BlockingState<'_> {
        BlockingState {
            is_waker: false,
            waker: self,
        }
    }

    /// Registers the current thread with an operation.
    #[inline]
    pub(crate) fn register(&self, oper: Operation, cx: &Context, state: &BlockingState<'_>) {
        self.inner.lock().unwrap().register(oper, cx);
        self.state.park(state.is_waker);
    }

    /// Unregisters an operation previously registered by the current thread.
    #[inline]
    pub(crate) fn unregister(&self, oper: Operation) -> Option<Entry> {
        self.inner.lock().unwrap().unregister(oper)
    }

    /// Attempts to find one thread (not the current one), select its operation, and wake it up.
    #[inline]
    pub(crate) fn notify(&self) {
        if self.state.try_notify() {
            self.notify_one()
        }
    }

    // Finds a thread (not the current one), select its operation, and wake it up.
    #[inline]
    pub(crate) fn notify_one(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.try_select();
        inner.notify();
    }

    /// Registers an operation waiting to be ready.
    #[inline]
    pub(crate) fn watch(&self, oper: Operation, cx: &Context) {
        self.inner.lock().unwrap().watch(oper, cx);
        self.state.park(false);
    }

    /// Unregisters an operation waiting to be ready.
    #[inline]
    pub(crate) fn unwatch(&self, oper: Operation) {
        let mut inner = self.inner.lock().unwrap();
        inner.unwatch(oper);
    }

    /// Notifies all threads that the channel is disconnected.
    #[inline]
    pub(crate) fn disconnect(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.disconnect();
    }
}

impl Drop for SyncWaker {
    #[inline]
    fn drop(&mut self) {
        debug_assert!(!self.state.has_waiters());
    }
}

/// A guard that manages the state of a blocking operation.
#[derive(Clone)]
struct BlockingState<'a> {
    /// True if this thread is the waker thread, meaning it must
    /// try to notify waiters after it completes.
    is_waker: bool,

    waker: &'a SyncWaker,
}

impl BlockingState<'_> {
    /// Reset the state after waking up from parking.
    #[inline]
    pub(crate) fn unpark(&mut self) {
        self.is_waker = self.waker.state.unpark();
    }

    /// Reset the state of an observer after waking up from parking.
    #[inline]
    pub(crate) fn unpark_observer(&mut self) {
        self.waker.state.unpark_observer();
    }
}

impl Drop for BlockingState<'_> {
    fn drop(&mut self) {
        if self.is_waker && self.waker.state.drop_waker() {
            self.waker.notify_one();
        }
    }
}

const NOTIFIED: u32 = 0b001;
const WAKER: u32 = 0b010;

/// The state of a `SyncWaker`.
struct WakerState {
    state: AtomicU32,
}

impl WakerState {
    /// Initialize the waker state.
    fn new() -> WakerState {
        WakerState {
            state: AtomicU32::new(0),
        }
    }

    /// Returns whether or not a waiter needs to be notified.
    fn try_notify(&self) -> bool {
        // because storing a value in the channel is also sequentially consistent,
        // this creates a total order between storing a value and registering a waiter.
        let state = self.state.load(Ordering::SeqCst);

        // if a notification is already set, the waker thread will take care
        // of further notifications. otherwise we have to notify if there are waiters
        if ((state >> NOTIFIED) & (state & NOTIFIED)) > 0 {
            return self
                .state
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |state| {
                    // set the notification if there are waiters and it is not already set
                    if (state >> WAKER) > 0 && (state & NOTIFIED == 0) {
                        Some(state | (WAKER | NOTIFIED))
                    } else {
                        None
                    }
                })
                .is_ok();
        }

        false
    }

    /// Get ready for this waker to park. The channel should be checked after calling this
    /// method, and before parking.
    fn park(&self, waker: bool) {
        // increment the waiter count. if we are the waker thread, we also have to remove the
        // notification to allow other waiters to be notified after we park
        let update = (1_u32 << WAKER).wrapping_sub(u32::from(waker));
        self.state.fetch_add(update, Ordering::SeqCst);
    }

    /// Remove an observer from the waker state after it was unparked.
    ///
    /// Observers never become the waking thread because if there were waiters, they
    /// are also woken up along with any observers.
    fn unpark_observer(&self) {
        self.state.fetch_sub(1_u32 << WAKER, Ordering::SeqCst);
    }

    /// Remove this waiter from the waker state after it was unparked.
    ///
    /// Returns `true` if this thread became the waking thread and must call `drop_waker`
    /// after it completes it's operation.
    fn unpark(&self) -> bool {
        self.state
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |state| {
                // decrement the waiter count and consume the waker token
                Some((state - (1 << WAKER)) & !WAKER)
            })
            // did we consume the token and become the waker thread?
            .map(|state| state & WAKER != 0)
            .unwrap()
    }

    /// Called by the waking thread after completing it's operation.
    ///
    /// Returns `true` if a waiter should be notified.
    fn drop_waker(&self) -> bool {
        self.state
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |state| {
                // if there are waiters, set the waker token and wake someone, transferring the
                // waker thread. otherwise unset the notification so new waiters can synchronize
                // with new notifications
                Some(if (state >> WAKER) > 0 {
                    state | WAKER
                } else {
                    state.wrapping_sub(NOTIFIED)
                })
            })
            // were there waiters?
            .map(|state| (state >> WAKER) > 0)
            .unwrap()
    }

    /// Returns `true` if there are active waiters.
    fn has_waiters(&self) -> bool {
        (self.state.load(Ordering::Relaxed) >> WAKER) > 0
    }
}

/// Returns the id of the current thread.
#[inline]
fn current_thread_id() -> ThreadId {
    std::thread_local! {
        /// Cached thread-local id.
        static THREAD_ID: ThreadId = thread::current().id();
    }

    THREAD_ID
        .try_with(|id| *id)
        .unwrap_or_else(|_| thread::current().id())
}
