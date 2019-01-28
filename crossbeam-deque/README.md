# Crossbeam Deque

[![Build Status](https://travis-ci.org/crossbeam-rs/crossbeam.svg?branch=master)](
https://travis-ci.org/crossbeam-rs/crossbeam)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](
https://github.com/crossbeam-rs/crossbeam-deque)
[![Cargo](https://img.shields.io/crates/v/crossbeam-deque.svg)](
https://crates.io/crates/crossbeam-deque)
[![Documentation](https://docs.rs/crossbeam-deque/badge.svg)](
https://docs.rs/crossbeam-deque)
[![Rust 1.28+](https://img.shields.io/badge/rust-1.28+-lightgray.svg)](
https://www.rust-lang.org)

This crate provides work-stealing deques, which are primarily intended for
building task schedulers.

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
crossbeam-deque = "0.7"
```

Next, add this to your crate:

```rust
extern crate crossbeam_deque;
```

## License

Licensed under either of

 * Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

#### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
