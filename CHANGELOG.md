# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](http://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.1] - 2018-07-07
### Changed
- Minor optimizations.

## [0.5.0] - 2018-07-06
### Added
- Add two deque constructors : `fifo()` and `lifo()`.

### Changed
- Update `rand` to `0.5.3`.
- Rename `Deque` to `Worker`.
- Return `Option<T>` from `Stealer::steal`.

### Removed
- Remove methods `Deque::len` and `Stealer::len`.
- Remove method `Deque::stealer`.
- Remove method `Deque::steal`.

## [0.4.1] - 2018-06-12
### Changed
- Update `crossbeam-epoch` to `0.5.0`.

## [0.4.0] - 2018-06-12
### Changed
- Update `crossbeam-epoch` to `0.4.2`.
- Update `crossbeam-utils` to `0.4.0`.
- Require minimum Rust version 1.25.

## [0.3.1] - 2018-05-04

### Added
- `Deque::capacity`
- `Deque::min_capacity`
- `Deque::shrink_to_fit`

### Changed
- Update `crossbeam-epoch` to `0.3.0`.
- Support Rust 1.20.
- Shrink the buffer in `Deque::push` if necessary.

## [0.3.0] - 2018-02-10

### Changed
- Update `crossbeam-epoch` to `0.4.0`.
- Drop support for Rust 1.13.

## [0.2.0] - 2018-02-10

### Changed
- Update `crossbeam-epoch` to `0.3.0`.
- Support Rust 1.13.

## [0.1.1] - 2017-11-29

### Changed
- Update `crossbeam-epoch` to `0.2.0`.

## 0.1.0 - 2017-11-26
### Added
- First implementation of the Chase-Lev deque.

[Unreleased]: https://github.com/crossbeam-rs/crossbeam-deque/compare/v0.5.1...HEAD
[0.5.1]: https://github.com/crossbeam-rs/crossbeam-deque/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/crossbeam-rs/crossbeam-deque/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/crossbeam-rs/crossbeam-deque/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/crossbeam-rs/crossbeam-deque/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/crossbeam-rs/crossbeam-deque/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/crossbeam-rs/crossbeam-deque/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/crossbeam-rs/crossbeam-deque/compare/v0.1.0...v0.2.0
[0.1.1]: https://github.com/crossbeam-rs/crossbeam-deque/compare/v0.1.0...v0.1.1
