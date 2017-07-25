use std::sync::atomic::{AtomicUsize, ATOMIC_USIZE_INIT};

use unprotected;
use participant::Participant;
use sync::list::{List, Entry};

// FIXME(stjepang): Participants are stored in a linked list because linked lists are fairly easy
// to implement in a lock-free manner. However, traversal is rather slow due to cache misses and
// data dependencies. We should experiment with other data structures as well.

thread_local! {
    /// The thread registration harness.
    ///
    /// The harness is lazily initialized on its first use, thus registrating the current thread.
    /// If initialized, the harness will get destructed on thread exit, which in turn unregisters
    /// the thread.
    static HARNESS: Harness = Harness {
        entry: unsafe {
            // Since we don't dereference any pointers in this block, it's okay to use
            // `unprotected`.
            unprotected(|scope| {
                participants().insert(Participant::new(), scope).as_raw()
            })
        }
    }
}

/// Holds a registered entry and unregisters it when dropped.
struct Harness {
    entry: *const Entry<Participant>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        unsafe {
            let entry = &*self.entry;

            // Unregister the thread by marking this entry as deleted.
            unprotected(|scope| entry.delete(scope));
        }
    }
}

/// Returns a reference to the head pointer of the list of participating threads.
fn participants() -> &'static List<Participant> {
    static PARTICIPANTS: AtomicUsize = ATOMIC_USIZE_INIT;
    unsafe { &*(&PARTICIPANTS as *const AtomicUsize as *const List<Participant>) }
}

/// Acquires a reference to the current participant.
///
/// Participants are lazily initialized on the first use.
///
/// # Panics
///
/// If this function is called while the thread is exiting, it might panic because it accesses
/// thread-local data.
pub fn with_current<F, R>(f: F) -> R
where
    F: FnOnce(&Participant) -> R,
{
    HARNESS.with(|harness| {
        let entry = unsafe { &*harness.entry };
        f(entry.get())
    })
}
