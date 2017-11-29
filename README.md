# Epoch-based garbage collection

[![Build Status](https://travis-ci.org/crossbeam-rs/crossbeam-epoch.svg?branch=master)](https://travis-ci.org/crossbeam-rs/crossbeam-epoch)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/crossbeam-rs/crossbeam-epoch)
[![Cargo](https://img.shields.io/crates/v/crossbeam-epoch.svg)](https://crates.io/crates/crossbeam-epoch)
[![Documentation](https://docs.rs/crossbeam-epoch/badge.svg)](https://docs.rs/crossbeam-epoch)

This crate provides epoch-based garbage collection for use in concurrent data structures.

If a thread removes a node from a concurrent data structure, other threads
may still have pointers to that node, so it cannot be immediately destructed.
Epoch GC allows deferring destruction until it becomes safe to do so.

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
crossbeam-epoch = "0.2"
```

Next, add this to your crate:

```rust
extern crate crossbeam_epoch as epoch;
```

## License

Licensed under the terms of MIT license and the Apache License (Version 2.0).

See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE) for details.
