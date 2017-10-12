/// Epoch-based garbage collector.
///
/// # Examples
///
/// ```
/// use crossbeam_epoch::Collector;
///
/// let collector = Collector::new();
///
/// let handle = collector.handle();
/// drop(collector); // `handle` still works after dropping `collector`
///
/// handle.pin(|scope| {
///     scope.flush();
/// });
/// ```

use std::sync::Arc;
use internal::{Global, Local};
use scope::Scope;

/// An epoch-based garbage collector.
pub struct Collector(Arc<Global>);

/// A handle to a garbage collector.
pub struct Handle {
    global: Arc<Global>,
    local: Local,
}

impl Collector {
    /// Creates a new collector.
    pub fn new() -> Self {
        Self { 0: Arc::new(Global::new()) }
    }

    /// Creates a new handle for the collector.
    #[inline]
    pub fn handle(&self) -> Handle {
        Handle::new(self.0.clone())
    }
}

impl Handle {
    fn new(global: Arc<Global>) -> Self {
        let local = Local::new(&global);
        Self { global, local }
    }

    /// Pin the current handle.
    #[inline]
    pub fn pin<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Scope) -> R,
    {
        unsafe { self.local.pin(&self.global, f) }
    }

    /// Check if the current handle is pinned.
    #[inline]
    pub fn is_pinned(&self) -> bool {
        self.local.is_pinned()
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { self.local.unregister(&self.global) }
    }
}
