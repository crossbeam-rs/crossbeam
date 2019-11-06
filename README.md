# Crossbeam

[![Build Status](https://travis-ci.org/crossbeam-rs/crossbeam.svg?branch=master)](
https://travis-ci.org/crossbeam-rs/crossbeam)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](
https://github.com/crossbeam-rs/crossbeam)
[![Cargo](https://img.shields.io/crates/v/crossbeam.svg)](
https://crates.io/crates/crossbeam)
[![Documentation](https://docs.rs/crossbeam/badge.svg)](
https://docs.rs/crossbeam)
[![Rust 1.28+](https://img.shields.io/badge/rust-1.28+-lightgray.svg)](
https://www.rust-lang.org)
[![chat](https://img.shields.io/discord/569610676205781012.svg?logo=discord)](https://discord.gg/JXYwgWZ)

This crate provides a set of tools for concurrent programming:

#### Atomics

* [`AtomicCell`], a thread-safe mutable memory location.<sup>(no_std)</sup>
* [`AtomicConsume`], for reading from primitive atomic types with "consume" ordering.<sup>(no_std)</sup>

#### Data structures

* [`deque`], work-stealing deques for building task schedulers.
* [`ArrayQueue`], a bounded MPMC queue that allocates a fixed-capacity buffer on construction.
* [`SegQueue`], an unbounded MPMC queue that allocates small buffers, segments, on demand.

#### Memory management

* [`epoch`], an epoch-based garbage collector.<sup>(alloc)</sup>

#### Thread synchronization

* [`channel`], multi-producer multi-consumer channels for message passing.
* [`Parker`], a thread parking primitive.
* [`ShardedLock`], a sharded reader-writer lock with fast concurrent reads.
* [`WaitGroup`], for synchronizing the beginning or end of some computation.

#### Utilities

* [`Backoff`], for exponential backoff in spin loops.<sup>(no_std)</sup>
* [`CachePadded`], for padding and aligning a value to the length of a cache line.<sup>(no_std)</sup>
* [`scope`], for spawning threads that borrow local variables from the stack.

*Features marked with <sup>(no_std)</sup> can be used in `no_std` environments.*<br/>
*Features marked with <sup>(alloc)</sup> can be used in `no_std` environments, but only if `alloc`
and `nightly` are enabled.*

[`AtomicCell`]: https://docs.rs/crossbeam/*/crossbeam/atomic/struct.AtomicCell.html
[`AtomicConsume`]: https://docs.rs/crossbeam/*/crossbeam/atomic/trait.AtomicConsume.html
[`deque`]: https://docs.rs/crossbeam/*/crossbeam/deque/index.html
[`ArrayQueue`]: https://docs.rs/crossbeam/*/crossbeam/queue/struct.ArrayQueue.html
[`SegQueue`]: https://docs.rs/crossbeam/*/crossbeam/queue/struct.SegQueue.html
[`channel`]: https://docs.rs/crossbeam/*/crossbeam/channel/index.html
[`Parker`]: https://docs.rs/crossbeam/*/crossbeam/sync/struct.Parker.html
[`ShardedLock`]: https://docs.rs/crossbeam/*/crossbeam/sync/struct.ShardedLock.html
[`WaitGroup`]: https://docs.rs/crossbeam/*/crossbeam/sync/struct.WaitGroup.html
[`epoch`]: https://docs.rs/crossbeam/*/crossbeam/epoch/index.html
[`Backoff`]: https://docs.rs/crossbeam/*/crossbeam/utils/struct.Backoff.html
[`CachePadded`]: https://docs.rs/crossbeam/*/crossbeam/utils/struct.CachePadded.html
[`scope`]: https://docs.rs/crossbeam/*/crossbeam/fn.scope.html

## Crates

The main `crossbeam` crate just [re-exports](src/lib.rs) tools from
smaller subcrates:

* [`crossbeam-channel`](crossbeam-channel)
  provides multi-producer multi-consumer channels for message passing.
* [`crossbeam-deque`](crossbeam-deque)
  provides work-stealing deques, which are primarily intended for building task schedulers.
* [`crossbeam-epoch`](crossbeam-epoch)
  provides epoch-based garbage collection for building concurrent data structures.
* [`crossbeam-queue`](crossbeam-queue)
  provides concurrent queues that can be shared among threads.
* [`crossbeam-utils`](crossbeam-utils)
  provides atomics, synchronization primitives, scoped threads, and other utilities.

There is one more experimental subcrate that is not yet included in `crossbeam`:

* [`crossbeam-skiplist`](crossbeam-skiplist)
  provides concurrent maps and sets based on lock-free skip lists.

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
crossbeam = "0.7"
```

Next, add this to your crate:

```rust
extern crate crossbeam;
```

## Compatibility

The minimum supported Rust version is 1.28. Any change to this is considered a breaking change.

## Contributing

Crossbeam welcomes contribution from everyone in the form of suggestions, bug reports,
pull requests, and feedback. 💛

If you need ideas for contribution, there are several ways to get started:

* Found a bug or have a feature request?
  [Submit an issue](https://github.com/crossbeam-rs/crossbeam/issues/new)!
* Issues and PRs labeled with
  [feedback wanted](https://github.com/crossbeam-rs/crossbeam/issues?utf8=%E2%9C%93&q=is%3Aopen+sort%3Aupdated-desc+label%3A%22feedback+wanted%22+)
  need feedback from users and contributors.
* Issues labeled with
  [good first issue](https://github.com/crossbeam-rs/crossbeam/issues?q=is%3Aissue+is%3Aopen+sort%3Aupdated-desc+label%3A%22good+first+issue%22)
  are relatively easy starter issues.

#### RFCs

We also have the [RFCs](https://github.com/crossbeam-rs/rfcs) repository for more
high-level discussion, which is the place where we brainstorm ideas and propose
substantial changes to Crossbeam.

You are welcome to participate in any open
[issues](https://github.com/crossbeam-rs/rfcs/issues?q=is%3Aissue+is%3Aopen+sort%3Aupdated-desc)
or
[pull requests](https://github.com/crossbeam-rs/rfcs/pulls?q=is%3Apr+is%3Aopen+sort%3Aupdated-desc).

#### Learning resources

If you'd like to learn more about concurrency and non-blocking data structures, there's a
list of learning resources in our [wiki](https://github.com/crossbeam-rs/rfcs/wiki),
which includes relevant blog posts, papers, videos, and other similar projects.

Another good place to visit is [merged RFCs](https://github.com/crossbeam-rs/rfcs/tree/master/text).
They contain elaborate descriptions and rationale for features we've introduced to
Crossbeam, but keep in mind that some of the written information is now out of date.

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

#### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
