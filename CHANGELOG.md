# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](http://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2017-11-17
### Added
- Add `nightly` feature.
- Use `repr(align(64))` on `CachePadded` with the `nightly` feature.
- Implement `Drop` for `CachePadded<T>`.
- Implement `Clone` for `CachePadded<T>`.
- Implement `From<T>` for `CachePadded<T>`.
- Implement better `Debug` for `CachePadded<T>`.
- Write more tests.
- Add this changelog.

### Changed
- Change cache line length to 64 bytes.

### Removed
- Remove `ZerosValid`.

## 0.1.0 - 2017-08-27
### Added
- Old implementation of `CachePadded` from `crossbeam` version 0.3.0

[Unreleased]: https://github.com/crossbeam-rs/crossbeam-utils/compare/0.2.0...HEAD
[0.2.0]: https://github.com/crossbeam-rs/crossbeam-utils/compare/0.1.0...0.2.0
