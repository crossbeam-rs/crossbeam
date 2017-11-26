# Multi-producer multi-consumer channels for message passing

[![Build Status](https://travis-ci.org/crossbeam-rs/crossbeam-channel.svg?branch=master)](https://travis-ci.org/crossbeam-rs/crossbeam-channel)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/crossbeam-rs/crossbeam-channel)
[![Cargo](https://img.shields.io/crates/v/crossbeam-channel.svg)](https://crates.io/crates/crossbeam-channel)
[![Documentation](https://docs.rs/crossbeam-channel/badge.svg)](https://docs.rs/crossbeam-channel)

Channels are concurrent FIFO queues used for passing messages between threads.

Crossbeam's channels are an alternative to the [`std::sync::mpsc`] channels
provided by the standard library. They are an improvement in pretty much all
aspects: ergonomics, flexibility, features, performance.

[`std::sync::mpsc`]: https://doc.rust-lang.org/std/sync/mpsc/index.html

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
crossbeam-channel = "0.1"
```

Next, add this to your crate:

```rust
extern crate crossbeam_channel;
```

## Comparison with `std::sync::mpsc`

|                                  | `std::sync::mpsc`                    | `crossbeam-channel`                               |
|----------------------------------|--------------------------------------|---------------------------------------------------|
| Unbounded channel constructor    | `std::sync::mpsc::channel()`         | `crossbeam_channel::unbounded()`                  |
| Bounded channel constructor      | `std::sync::mpsc::sync_channel(cap)` | `crossbeam_channel::bounded(cap)`                 |
| Sender types                     | `Sender` and `SyncSender`            | `Sender`                                          |
| Receiver types                   | `Receiver`                           | `Receiver`                                        |
| Sender implements `Sync`?        | No                                   | Yes                                               |
| Receiver implements `Sync`?      | No                                   | Yes                                               |
| Sender implements `Clone`?       | Yes                                  | Yes                                               |
| Receiver implements `Clone`?     | No                                   | Yes                                               |
| Sender operations                | `try_send`, `send`                   | `try_send`, `send`, `send_timeout`                |
| Receiver operations              | `try_recv`, `recv`, `recv_timeout`   | `try_recv`, `recv`, `recv_timeout`                |
| Additional sender methods        |                                      | `is_empty`, `len`, `capacity`                     |
| Additional receiver methods      | `iter`, `try_iter`                   | `iter`, `try_iter`, `is_empty`, `len`, `capacity` |
| Selection macro                  | `select!`                            | `select_loop!`                                    |
| Select interface                 | `std::sync::mpsc::Select`            | `crossbeam_channel::Select`                       |
| Select has a safe interface?     | No                                   | Yes                                               |
| Select supports send operations? | No                                   | Yes                                               |
| Select can have a timeout?       | No                                   | Yes                                               |
| Select can be non-blocking?      | No                                   | Yes                                               |

## License

Licensed under the terms of MIT license and the Apache License (Version 2.0).

See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE) for details.
