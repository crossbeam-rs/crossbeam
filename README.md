# Crossbeam: support for concurrent and parallel programming

[![Build Status](https://travis-ci.org/aturon/crossbeam.svg?branch=master)](https://travis-ci.org/aturon/crossbeam)

This crate is an early work in progress. The focus for the moment is
concurrency:

- **Non-blocking data structures**. These data structures allow for high
performance, highly-concurrent access, much superior to wrapping with a
`Mutex`. Ultimately the goal is to include stacks, queues, deques, bags, sets
and maps.

- **Memory management**. Because non-blocking data structures avoid global
synchronization, it is not easy to tell when internal data can be safely
freed. The `mem` module provides generic, easy to use, and high-performance APIs
for managing memory in these cases.

- **Synchronization**. The standard library provides a few synchronization
primitives (locks, semaphores, barriers, etc) but this crate seeks to expand
that set to include more advanced/niche primitives, as well as userspace
alternatives.

- **Scoped thread API**. Finally, the crate provides a "scoped" thread API,
making it possible to spawn threads that share stack data with their parents.

# Usage

To use Crossbeam, add this to your `Cargo.toml`:

```toml
[dependencies]
crossbeam = "0.2"
```

For examples of what Crossbeam is capable of, see the
[documentation][docs].

[docs]: http://aturon.github.io/crossbeam-doc/crossbeam/
