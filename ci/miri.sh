#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"/..

export RUSTFLAGS="${RUSTFLAGS:-} -Z randomize-layout"

# disable preemption due to https://github.com/rust-lang/rust/issues/55005
MIRIFLAGS="-Zmiri-strict-provenance -Zmiri-symbolic-alignment-check -Zmiri-disable-isolation -Zmiri-preemption-rate=0" \
    cargo miri test \
    -p crossbeam-queue \
    -p crossbeam-utils

# -Zmiri-ignore-leaks is needed because we use detached threads in tests/docs: https://github.com/rust-lang/miri/issues/1371
# disable preemption due to https://github.com/rust-lang/rust/issues/55005
MIRIFLAGS="-Zmiri-strict-provenance -Zmiri-symbolic-alignment-check -Zmiri-disable-isolation -Zmiri-ignore-leaks -Zmiri-preemption-rate=0" \
    cargo miri test \
    -p crossbeam-channel

# -Zmiri-ignore-leaks is needed for https://github.com/crossbeam-rs/crossbeam/issues/579
# -Zmiri-disable-stacked-borrows is needed for https://github.com/crossbeam-rs/crossbeam/issues/545
# disable preemption due to https://github.com/rust-lang/rust/issues/55005
MIRIFLAGS="-Zmiri-check-number-validity -Zmiri-symbolic-alignment-check -Zmiri-disable-isolation -Zmiri-disable-stacked-borrows -Zmiri-ignore-leaks -Zmiri-preemption-rate=0" \
    cargo miri test \
    -p crossbeam-epoch \
    -p crossbeam-skiplist

# -Zmiri-ignore-leaks is needed for https://github.com/crossbeam-rs/crossbeam/issues/579
# -Zmiri-disable-stacked-borrows is needed for https://github.com/crossbeam-rs/crossbeam/issues/545
# disable preemption due to https://github.com/rust-lang/rust/issues/55005
MIRIFLAGS="-Zmiri-check-number-validity -Zmiri-symbolic-alignment-check -Zmiri-disable-stacked-borrows -Zmiri-ignore-leaks -Zmiri-compare-exchange-weak-failure-rate=0.0 -Zmiri-preemption-rate=0" \
    cargo miri test \
    -p crossbeam-deque

# -Zmiri-ignore-leaks is needed for https://github.com/crossbeam-rs/crossbeam/issues/579
# disable preemption due to https://github.com/rust-lang/rust/issues/55005
MIRIFLAGS="-Zmiri-check-number-validity -Zmiri-symbolic-alignment-check -Zmiri-ignore-leaks -Zmiri-preemption-rate=0" \
    cargo miri test \
    -p crossbeam
