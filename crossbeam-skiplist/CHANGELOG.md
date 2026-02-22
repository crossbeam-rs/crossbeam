# Version 0.2.0

- Bump the minimum supported Rust version to 1.74.
- Improve compatibility with Miri:
  - Fix stacked borrows violations. (#871)
- Allow lookup to be customized. (#1132)
- Support custom comparators. (#1182)
- Explicitly annotate lifetime of `{Entry,RefEntry}::{key,value}`. (#1141)
- Fix #1023. (#1101)
- Fix memory leaks in `{map,set}::Range`/`base::RefRange`. (#1217)
- Update `crossbeam-epoch` to 0.10.
- Update `crossbeam-utils` to 0.9.

TODO:
- entry for #1101 should be more descriptive.
- mention https://github.com/crossbeam-rs/crossbeam/pull/1143
- consider (non-breaking) https://github.com/crossbeam-rs/crossbeam/pull/1091

# Version 0.1.3

- Remove dependency on `cfg-if`. (#1072)

# Version 0.1.2

- Bump the minimum supported Rust version to 1.61. (#1037)
- Add `compare_insert`. (#976)
- Improve support for targets without atomic CAS. (#1037)
- Remove build script. (#1037)
- Remove dependency on `scopeguard`. (#1045)

# Version 0.1.1

- Fix `get_unchecked` panic by raw pointer calculation. (#940)

# Version 0.1.0

**Note:** This release has been yanked due to bug fixed in 0.1.1.

- Initial implementation.
