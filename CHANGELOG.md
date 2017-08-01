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
