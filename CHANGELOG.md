# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](http://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/crossbeam-rs/crossbeam-epoch/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/crossbeam-rs/crossbeam-epoch/compare/v0.1.0...v0.2.0
