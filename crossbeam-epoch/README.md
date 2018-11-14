# Epoch-based garbage collection

[![Build Status](https://travis-ci.org/crossbeam-rs/crossbeam.svg?branch=master)](
https://travis-ci.org/crossbeam-rs/crossbeam)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](
https://github.com/crossbeam-rs/crossbeam-epoch)
[![Cargo](https://img.shields.io/crates/v/crossbeam-epoch.svg)](
https://crates.io/crates/crossbeam-epoch)
[![Documentation](https://docs.rs/crossbeam-epoch/badge.svg)](
https://docs.rs/crossbeam-epoch)

This crate provides epoch-based garbage collection for use in concurrent data structures.

If a thread removes a node from a concurrent data structure, other threads
may still have pointers to that node, so it cannot be immediately destructed.
Epoch GC allows deferring destruction until it becomes safe to do so.

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
crossbeam-epoch = "0.6"
```

Next, add this to your crate:

```rust
extern crate crossbeam_epoch as epoch;
```

The minimum required Rust version is 1.26.

## License

Licensed under either of

 * Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
