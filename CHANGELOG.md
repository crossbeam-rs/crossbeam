# Changelog
All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](http://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/crossbeam-rs/crossbeam-channel/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/crossbeam-rs/crossbeam-channel/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/crossbeam-rs/crossbeam-channel/compare/v0.1.0...v0.1.1
