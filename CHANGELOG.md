# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](http://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.4] - 2018-08-02
### Changed
- Make `select!` linearizable with other channel operations.
- Update `crossbeam-utils` to `0.5.0`.
- Update `parking_lot` to `0.6.3`.

### Removed
- Remove Mac OS X tests.

## [0.2.3] - 2018-07-21
### Added
- Add Mac OS X tests.

### Changed
- Lower some memory orderings.

### Fixed
- Eliminate calls to `mem::unitialized`, which caused bugs with ZST.

## [0.2.2] - 2018-07-10
### Added
- Add more tests.

### Changed
- Update `crossbeam-epoch` to 0.5.0
- Initialize the RNG seed to a random value.
- Replace `libc::abort` with `std::process::abort`.

### Fixed
- Ignore clippy warnings in `select!`.
- Better interaction of `select!` with the NLL borrow checker.

## [0.2.1] - 2018-06-12
### Changed
- Fix compilation errors when using `select!` with `#[deny(unsafe_code)]`.

## [0.2.0] - 2018-06-11
### Added
- Implement `IntoIterator<Item = T>` for `Receiver<T>`.
- Add a new `select!` macro.
- Add special channels `after` and `tick`.

### Changed
- Dropping receivers doesn't close the channel anymore.
- Change the signature of `recv`, `send`, and `try_recv`.

### Removed
- Remove `Sender::is_closed` and `Receiver::is_closed`.
- Remove `Sender::close` and `Receiver::close`.
- Remove `Sender::send_timeout` and `Receiver::recv_timeout`.
- Remove `Sender::try_send`.
- Remove `Select` and `select_loop!`.
- Remove all error types.
- Remove `Iter`, `TryIter`, and `IntoIter`.
- Remove the `nightly` feature.
- Remove ordering operators for `Sender` and `Receiver`.

## [0.1.3] - 2018-05-23
### Added
- Add `Sender::disconnect` and `Receiver::disconnect`.
- Implement comparison operators for `Sender` and `Receiver`.
- Allow arbitrary patterns in place of `msg` in `recv(r, msg)`.
- Add a few conversion impls between error types.
- Add benchmarks for `atomicring` and `mpmc`.
- Add benchmarks for different message sizes.

### Changed
- Documentation improvements.
- Update `crossbeam-epoch` to 0.4.0
- Update `crossbeam-utils` to 0.3.0
- Update `parking_lot` to 0.5
- Update `rand` to 0.4

## [0.1.2] - 2017-12-12
### Added
- Allow conditional cases in `select_loop!` macro.

### Fixed
- Fix typos in documentation.
- Fix deadlock in selection when all channels are disconnected and a timeout is specified.

## [0.1.1] - 2017-11-27
### Added
- Implement `Debug` for `Sender`, `Receiver`, `Iter`, `TryIter`, `IntoIter`, and `Select`.
- Implement `Default` for `Select`.

## 0.1.0 - 2017-11-26
### Added
- First implementation of the channels.
- Add `select_loop!` macro by @TimNN.

[Unreleased]: https://github.com/crossbeam-rs/crossbeam-channel/compare/v0.2.4...HEAD
[0.2.4]: https://github.com/crossbeam-rs/crossbeam-channel/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/crossbeam-rs/crossbeam-channel/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/crossbeam-rs/crossbeam-channel/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/crossbeam-rs/crossbeam-channel/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/crossbeam-rs/crossbeam-channel/compare/v0.1.3...v0.2.0
[0.1.3]: https://github.com/crossbeam-rs/crossbeam-channel/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/crossbeam-rs/crossbeam-channel/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/crossbeam-rs/crossbeam-channel/compare/v0.1.0...v0.1.1
