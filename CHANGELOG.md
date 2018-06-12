# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](http://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.3] - 2018-06-12
## Changed
- Downgrade `crossbeam-utils` to 0.3 because it was a breaking change.

## [0.4.2] - 2018-06-12
### Added
- Expose the `Pointer` trait.
- Warn missing docs and missing debug impls.

## Changed
- Update `crossbeam-utils` to 0.4.

## [0.4.1] - 2018-03-20
### Added
- Add `Debug` impls for `Collector`, `Handle`, and `Guard`.
- Add `load_consume` to `Atomic`.

### Changed
- Rename `Collector::handle` to `Collector::register`.

### Fixed
- Remove the `Send` implementation for `Handle` (this was a bug). Only
  `Collector`s can be shared among multiple threads, while `Handle`s and
  `Guard`s must stay within the thread in which they were created.

## [0.4.0] - 2018-02-10
### Changed
- Update dependencies.

### Removed
- Remove support for Rust 1.13.

## [0.3.0] - 2018-02-10
### Added
- Add support for Rust 1.13.

### Changed
- Improve documentation for CAS.

## [0.2.0] - 2017-11-29
### Added
- Add method `Owned::into_box`.

### Changed
- Fix a use-after-free bug in `Local::finalize`.
- Fix an ordering bug in `Global::push_bag`.
- Fix a bug in calculating distance between epochs.

### Removed
- Remove `impl<T> Into<Box<T>> for Owned<T>`.

## 0.1.0 - 2017-11-26
### Added
- First version of the new epoch-based GC.

[Unreleased]: https://github.com/crossbeam-rs/crossbeam-epoch/compare/v0.4.3...HEAD
[0.4.3]: https://github.com/crossbeam-rs/crossbeam-epoch/compare/v0.4.2...v0.4.3
[0.4.2]: https://github.com/crossbeam-rs/crossbeam-epoch/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/crossbeam-rs/crossbeam-epoch/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/crossbeam-rs/crossbeam-epoch/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/crossbeam-rs/crossbeam-epoch/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/crossbeam-rs/crossbeam-epoch/compare/v0.1.0...v0.2.0
