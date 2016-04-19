// Manage the thread-local state, providing access to a `Participant` record.

use std::sync::atomic::Ordering::Relaxed;

use mem::epoch::participant::Participant;
use mem::epoch::global;

#[derive(Debug)]
struct LocalEpoch {
    participant: *const Participant,
}

impl LocalEpoch {
    fn new() -> LocalEpoch {
        LocalEpoch { participant: global::get().participants.enroll() }
    }

    fn get(&self) -> &Participant {
        unsafe { &*self.participant }
    }
}

// FIXME: avoid leaking when all threads have exited
impl Drop for LocalEpoch {
    fn drop(&mut self) {
        let p = self.get();
        p.enter();
        p.migrate_garbage();
        p.exit();
        p.active.store(false, Relaxed);
    }
}

thread_local!(static LOCAL_EPOCH: LocalEpoch = LocalEpoch::new() );

pub fn with_participant<F, T>(f: F) -> T where F: FnOnce(&Participant) -> T {
    LOCAL_EPOCH.with(|e| f(e.get()))
}
