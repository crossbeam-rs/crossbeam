# Crossbeam

[![Build Status](https://travis-ci.org/crossbeam-rs/crossbeam.svg?branch=master)](
https://travis-ci.org/crossbeam-rs/crossbeam)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](
https://github.com/crossbeam-rs/crossbeam)
[![Cargo](https://img.shields.io/crates/v/crossbeam.svg)](
https://crates.io/crates/crossbeam)
[![Documentation](https://docs.rs/crossbeam/badge.svg)](
https://docs.rs/crossbeam)
[![Rust 1.26+](https://img.shields.io/badge/rust-1.26+-lightgray.svg)](
https://www.rust-lang.org)

This crate provides a set of tools for concurrent programming:

* Atomics
    * `ArcCell<T>` is a shared mutable `Arc<T>` pointer.
    * `AtomicCell<T>` is equivalent to `Cell<T>`, except it is also thread-safe.
    * `AtomicConsume` allows reading from primitive atomic types with "consume" ordering.

* Data structures
    * `deque` module contains work-stealing deques for building task schedulers.
    * `MsQueue<T>` and `SegQueue<T>` are simple concurrent queues.
    * `TreiberStack<T>` is a lock-free stack.

* Synchronization
    * `channel` module contains multi-producer multi-consumer channels for message passing.
    * `ShardedLock<T>` is like `RwLock<T>`, but sharded for faster concurrent reads.
    * `WaitGroup` enables threads to synchronize the beginning or end of some computation.

* Memory management
    * `epoch` module contains epoch-based garbage collection.

* Utilities
    * `CachePadded<T>` pads and aligns a value to the length of a cache line.
    * `scope()` can spawn threads that borrow local variables from the stack. 

## Crates

Some of the tools live in the main `crossbeam` crate, and some are re-exported
from smaller subcrates:

* [`crossbeam-channel`](crossbeam-channel)
  provides multi-producer multi-consumer channels for message passing.
* [`crossbeam-deque`](crossbeam-deque)
  provides work-stealing deques, which are primarily intended for building task schedulers.
* [`crossbeam-epoch`](crossbeam-epoch)
  provides epoch-based garbage collection for building concurrent data structures.
* [`crossbeam-utils`](crossbeam-utils)
  provides miscellaneous utilities for concurrent programming:

Take a look at [src/lib.rs](src/lib.rs) to see what goes where.

There is one more experimental subcrate that is not yet included in `crossbeam`:

* [`crossbeam-skiplist`](crossbeam-skiplist)
  provides concurrent maps and sets based on lock-free skip lists.

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
crossbeam = "0.6"
```

Next, add this to your crate:

```rust
extern crate crossbeam;
```

## Compatibility

The minimum supported Rust version is 1.26.

Features available in `no_std` environments:

* `AtomicCell<T>`
* `AtomicConsume`
* `CachePadded<T>`
* `epoch` (nightly Rust only)

## Contributing

Crossbeam welcomes contribution from everyone in the form of suggestions, bug reports,
pull requests, and feedback. 💛

If you're looking for things to do, there are several easy ways to get started:

* Found a bug or have a feature request?
  [Tell us](https://github.com/crossbeam-rs/crossbeam/issues/new)!
* Issues and PRs labeled with
  [feedback wanted](https://github.com/crossbeam-rs/crossbeam/issues?utf8=%E2%9C%93&q=is%3Aopen+sort%3Aupdated-desc+label%3A%22feedback+wanted%22+)
  need feedback from users and contributors.
* Issues labeled with
  [good first issue](https://github.com/crossbeam-rs/crossbeam/issues?q=is%3Aissue+is%3Aopen+sort%3Aupdated-desc+label%3A%22good+first+issue%22)
  are relatively easy starter issues.

#### RFCs

We also have the [RFCs](https://github.com/crossbeam-rs/rfcs) repository for more
high-level discussion. It is a place where we brainstorm ideas and propose
substantial changes to Crossbeam.

Feel free to participate in any open 
[issues](https://github.com/crossbeam-rs/rfcs/issues?q=is%3Aissue+is%3Aopen+sort%3Aupdated-desc)
or
[pull requests](https://github.com/crossbeam-rs/rfcs/pulls?q=is%3Apr+is%3Aopen+sort%3Aupdated-desc)!

#### Learning resources

If you'd like to learn more about concurrency and non-blocking data structures, there's a
list of learning resources in our [wiki](https://github.com/crossbeam-rs/rfcs/wiki),
which includes related blog posts, papers, videos, and other similar projects.

Another good place to visit is [merged RFCs](https://github.com/crossbeam-rs/rfcs/tree/master/text).
They contain elaborate descriptions and rationale for features we've introduced to
Crossbeam, but note that some of the written information is now out of date.

#### Conduct

The Crossbeam project adheres to the
[Rust Code of Conduct](https://github.com/rust-lang/rust/blob/master/CODE_OF_CONDUCT.md).
This describes the minimum behavior expected from all contributors.

## License

Licensed under either of

 * Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

Some Crossbeam subcrates have additional licensing notices.
Take a look at other readme files in this repository for more information.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
