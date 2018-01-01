# Notice

This crate is outdated. The Crossbeam project is currently in a transition
period. We are rewriting the epoch garbage collector, as well as several
other utilities and adding new structures. To follow the progress, please
take a look at other crates in the project [here](https://github.com/crossbeam-rs).
When the transition is complete, this crate will be updated to use the new code.

# Crossbeam: support for concurrent and parallel programming

[![Build Status](https://travis-ci.org/crossbeam-rs/crossbeam.svg?branch=master)](https://travis-ci.org/crossbeam-rs/crossbeam)


This crate is an early work in progress. The focus for the moment is
concurrency:

- **Non-blocking data structures**. These data structures allow for high
performance, highly-concurrent access, much superior to wrapping with a
`Mutex`. Ultimately the goal is to include stacks, queues, deques, bags, sets
and maps.

- **Memory management**. Because non-blocking data structures avoid global
synchronization, it is not easy to tell when internal data can be safely
freed. The `epoch` module provides generic, easy to use, and high-performance APIs
for managing memory in these cases.

- **Synchronization**. The standard library provides a few synchronization
primitives (locks, barriers, etc) but this crate seeks to expand that set to
include more advanced/niche primitives, as well as userspace alternatives.

- **Scoped thread API**. Finally, the crate provides a "scoped" thread API,
making it possible to spawn threads that share stack data with their parents.

# Usage

To use Crossbeam, add this to your `Cargo.toml`:

```toml
[dependencies]
crossbeam = "0.3.0"
```

For examples of what Crossbeam is capable of, see the
[documentation][docs].

[docs]: https://docs.rs/crossbeam/
