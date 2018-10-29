# Version 0.5.0

- Reorganize sub-modules and rename functions.

# Version 0.4.1

- Fix a documentation link.

# Version 0.4.0

- `CachePadded` supports types bigger than 64 bytes.
- Fix a bug in scoped threads where unitialized memory was being dropped.
- Minimum required Rust version is now 1.25.

# Version 0.3.2

- Mark `load_consume` with `#[inline]`.

# Version 0.3.1

- `load_consume` on ARM and AArch64.

# Version 0.3.0

- Add `join` for scoped thread API.
- Add `load_consume` for atomic load-consume memory ordering.
- Remove `AtomicOption`.

# Version 0.2.2

- Support Rust 1.12.1.
- Call `T::clone` when cloning a `CachePadded<T>`.

# Version 0.2.1

- Add `use_std` feature.

# Version 0.2.0

- Add `nightly` feature.
- Use `repr(align(64))` on `CachePadded` with the `nightly` feature.
- Implement `Drop` for `CachePadded<T>`.
- Implement `Clone` for `CachePadded<T>`.
- Implement `From<T>` for `CachePadded<T>`.
- Implement better `Debug` for `CachePadded<T>`.
- Write more tests.
- Add this changelog.
- Change cache line length to 64 bytes.
- Remove `ZerosValid`.

# Version 0.1.0

- Old implementation of `CachePadded` from `crossbeam` version 0.3.0
