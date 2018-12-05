# Crossbeam Utils

[![Build Status](https://travis-ci.org/crossbeam-rs/crossbeam.svg?branch=master)](
https://travis-ci.org/crossbeam-rs/crossbeam)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](
https://github.com/crossbeam-rs/crossbeam-utils/tree/master/src)
[![Cargo](https://img.shields.io/crates/v/crossbeam-utils.svg)](
https://crates.io/crates/crossbeam-utils)
[![Documentation](https://docs.rs/crossbeam-utils/badge.svg)](
https://docs.rs/crossbeam-utils)
[![Rust 1.26+](https://img.shields.io/badge/rust-1.26+-lightgray.svg)](
https://www.rust-lang.org)

This crate provides miscellaneous utilities for concurrent programming:

* `AtomicConsume` allows reading from primitive atomic types with "consume" ordering.
* `CachePadded<T>` pads and aligns a value to the length of a cache line.
* `scope()` can spawn threads that borrow local variables from the stack. 

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
crossbeam-utils = "0.6"
```

Next, add this to your crate:

```rust
extern crate crossbeam_utils;
```

## Compatibility

The minimum supported Rust version is 1.26.

Features available in `no_std` environments:

* `AtomicConsume`
* `CachePadded<T>`

## License

Licensed under either of

 * Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
