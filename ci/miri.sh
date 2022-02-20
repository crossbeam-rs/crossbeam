#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"/..

export RUSTFLAGS="${RUSTFLAGS:-} -Z randomize-layout"

MIRIFLAGS="-Zmiri-check-number-validity -Zmiri-symbolic-alignment-check -Zmiri-tag-raw-pointers" \
    cargo miri test \
    -p crossbeam-queue

# -Zmiri-tag-raw-pointers doesn't work with std::thread::Builder::name on Linux: https://github.com/rust-lang/miri/issues/1717
MIRIFLAGS="-Zmiri-check-number-validity -Zmiri-symbolic-alignment-check -Zmiri-disable-isolation" \
    cargo miri test \
    -p crossbeam-utils

# -Zmiri-ignore-leaks is needed because we use detached threads in tests/docs: https://github.com/rust-lang/miri/issues/1371
MIRIFLAGS="-Zmiri-check-number-validity -Zmiri-symbolic-alignment-check -Zmiri-tag-raw-pointers -Zmiri-disable-isolation -Zmiri-ignore-leaks" \
    cargo miri test \
    -p crossbeam-channel

# -Zmiri-ignore-leaks is needed for https://github.com/crossbeam-rs/crossbeam/issues/579
# -Zmiri-disable-stacked-borrows is needed for https://github.com/crossbeam-rs/crossbeam/issues/545
MIRIFLAGS="-Zmiri-check-number-validity -Zmiri-symbolic-alignment-check -Zmiri-disable-isolation -Zmiri-disable-stacked-borrows -Zmiri-ignore-leaks" \
    cargo miri test \
    -p crossbeam-epoch \
    -p crossbeam-skiplist

# -Zmiri-ignore-leaks is needed for https://github.com/crossbeam-rs/crossbeam/issues/579
# -Zmiri-disable-stacked-borrows is needed for https://github.com/crossbeam-rs/crossbeam/issues/545
MIRIFLAGS="-Zmiri-check-number-validity -Zmiri-symbolic-alignment-check -Zmiri-disable-stacked-borrows -Zmiri-ignore-leaks -Zmiri-compare-exchange-weak-failure-rate=1.0" \
    cargo miri test \
    -p crossbeam-deque

# -Zmiri-ignore-leaks is needed for https://github.com/crossbeam-rs/crossbeam/issues/579
MIRIFLAGS="-Zmiri-check-number-validity -Zmiri-symbolic-alignment-check -Zmiri-ignore-leaks" \
    cargo miri test \
    -p crossbeam
