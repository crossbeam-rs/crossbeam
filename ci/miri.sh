#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"/..

MIRIFLAGS="-Zmiri-tag-raw-pointers" \
    cargo miri test \
        -p crossbeam-queue

# -Zmiri-tag-raw-pointers doesn't work with std::thread::Builder::name on Linux: https://github.com/rust-lang/miri/issues/1717
MIRIFLAGS="-Zmiri-disable-isolation" \
    cargo miri test \
        -p crossbeam-utils

# -Zmiri-ignore-leaks is needed because we use detached threads in tests/docs: https://github.com/rust-lang/miri/issues/1371
MIRIFLAGS="-Zmiri-tag-raw-pointers -Zmiri-disable-isolation -Zmiri-ignore-leaks" \
    cargo miri test \
        -p crossbeam-channel

# -Zmiri-ignore-leaks is needed for https://github.com/crossbeam-rs/crossbeam/issues/579
# -Zmiri-disable-stacked-borrows is needed for https://github.com/crossbeam-rs/crossbeam/issues/545
MIRIFLAGS="-Zmiri-disable-isolation -Zmiri-ignore-leaks -Zmiri-disable-stacked-borrows" \
    cargo miri test \
        -p crossbeam-epoch \
        -p crossbeam-skiplist

# -Zmiri-ignore-leaks is needed for https://github.com/crossbeam-rs/crossbeam/issues/579
# -Zmiri-disable-stacked-borrows is needed for https://github.com/crossbeam-rs/crossbeam/issues/545
MIRIFLAGS="-Zmiri-ignore-leaks -Zmiri-disable-stacked-borrows -Zmiri-compare-exchange-weak-failure-rate=1.0" \
    cargo miri test \
        -p crossbeam-deque

# -Zmiri-ignore-leaks is needed for https://github.com/crossbeam-rs/crossbeam/issues/579
MIRIFLAGS="-Zmiri-ignore-leaks" \
    cargo miri test \
        -p crossbeam
