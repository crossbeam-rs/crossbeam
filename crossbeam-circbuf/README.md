# Concurrent queues based on circular buffer

[![Build Status](https://travis-ci.org/crossbeam-rs/crossbeam.svg?branch=master)](
https://travis-ci.org/crossbeam-rs/crossbeam)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](
https://github.com/crossbeam-rs/crossbeam-circbuf)
[![Cargo](https://img.shields.io/crates/v/crossbeam-circbuf.svg)](
https://crates.io/crates/crossbeam-circbuf)
[![Documentation](https://docs.rs/crossbeam-circbuf/badge.svg)](
https://docs.rs/crossbeam-circbuf)
[![Rust 1.26+](https://img.shields.io/badge/rust-1.26+-lightgray.svg)](
https://www.rust-lang.org)

This crate provides concurrent queues based on circular buffer.

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
crossbeam-circbuf = "0.1"
```

Next, add this to your crate:

```rust
extern crate crossbeam_circbuf as circbuf;
```

## Compatibility

The minimum supported Rust version is 1.26.

## License

Licensed under either of

 * Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
