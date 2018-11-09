# Version 0.5.0

- Update `crossbeam-channel` to 0.3.
- Update `crossbeam-utils` to 0.6.
- Add `AtomicCell`, `SharedLock`, and `WaitGroup`.

# Version 0.4.1

- Fix a double-free bug in `MsQueue` and `SegQueue`.

# Version 0.4

- Switch to the new implementation of epoch-based reclamation in
  [`crossbeam-epoch`](https://github.com/crossbeam-rs/crossbeam-epoch), fixing numerous bugs in the
  old implementation.  Its API is changed in a backward-incompatible way.
- Switch to the new implementation of `CachePadded` and scoped thread in
  [`crossbeam-utils`](https://github.com/crossbeam-rs/crossbeam-utils).  The scoped thread API is
  changed in a backward-incompatible way.
- Switch to the new implementation of Chase-Lev deque in
  [`crossbeam-deque`](https://github.com/crossbeam-rs/crossbeam-deque).  Its API is changed in a
  backward-incompatible way.
- Export channel implemented in
  [`crossbeam-channel`](https://github.com/crossbeam-rs/crossbeam-channel).
- Remove `AtomicOption`.
- Implement `Default` and `From` traits.

# Version 0.3

- Introduced `ScopedThreadBuilder` with the ability to name threads and set stack size
- `Worker` methods in the Chase-Lev deque don't require mutable access anymore
- Fixed a bug when unblocking `pop()` in `MsQueue`
- Implemented `Drop` for `MsQueue`, `SegQueue`, and `TreiberStack`
- Implemented `Default` for `TreiberStack`
- Added `is_empty` to `SegQueue`
- Renamed `mem::epoch` to `epoch`
- Other bug fixes

# Version 0.2

- Changed existing non-blocking `pop` methods to `try_pop`
- Added blocking `pop` support to Michael-Scott queue
- Added Chase-Lev work-stealing deque

# Version 0.1

- Added [epoch-based memory management](http://aturon.github.io/blog/2015/08/27/epoch/)
- Added Michael-Scott queue
- Added Segmented array queue
