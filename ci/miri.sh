#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"/..

group=$1

# We need 'ts' for the per-line timing
sudo apt-get -y install moreutils
echo

export RUSTFLAGS="${RUSTFLAGS:-} -Z randomize-layout"
export RUSTDOCFLAGS="${RUSTDOCFLAGS:-} -Z randomize-layout"
export MIRIFLAGS="${MIRIFLAGS:-} -Zmiri-strict-provenance -Zmiri-symbolic-alignment-check -Zmiri-disable-isolation"

case "${group}" in
    channel)
        # -Zmiri-ignore-leaks is needed because we use detached threads in tests in tests/array.rs: panic_on_drop
        MIRI_LEAK_CHECK='1' \
        MIRIFLAGS="${MIRIFLAGS} -Zmiri-ignore-leaks" \
            cargo miri test --all-features \
            -p crossbeam-channel 2>&1 | ts -i '%.s  '
        # -Zmiri-ignore-leaks is needed because we use detached threads in tests in tests/golang.rs: https://github.com/rust-lang/miri/issues/1371
        MIRI_LEAK_CHECK='1' \
        MIRIFLAGS="${MIRIFLAGS} -Zmiri-ignore-leaks" \
            cargo miri test --all-features \
            -p crossbeam-channel --test golang 2>&1 | ts -i '%.s  '
        ;;
    others)
        cargo miri test --all-features \
            -p crossbeam-queue \
            -p crossbeam-utils 2>&1 | ts -i '%.s  '
        # Use Tree Borrows instead of Stacked Borrows because epoch is not compatible with Stacked Borrows: https://github.com/crossbeam-rs/crossbeam/issues/545#issuecomment-1192785003
        MIRIFLAGS="${MIRIFLAGS} -Zmiri-tree-borrows" \
            cargo miri test --all-features \
            -p crossbeam-epoch \
            -p crossbeam-skiplist \
            -p crossbeam 2>&1 | ts -i '%.s  '
        # Use Tree Borrows instead of Stacked Borrows because epoch is not compatible with Stacked Borrows: https://github.com/crossbeam-rs/crossbeam/issues/545#issuecomment-1192785003
        # -Zmiri-compare-exchange-weak-failure-rate=0.0 is needed because some sequential tests (e.g.,
        # doctest of Stealer::steal) incorrectly assume that sequential weak CAS will never fail.
        # -Zmiri-preemption-rate=0 is needed because this code technically has UB and Miri catches that.
        MIRIFLAGS="${MIRIFLAGS} -Zmiri-tree-borrows -Zmiri-compare-exchange-weak-failure-rate=0.0 -Zmiri-preemption-rate=0" \
            cargo miri test --all-features \
            -p crossbeam-deque 2>&1 | ts -i '%.s  '
        ;;
    *)
        echo "unknown crate group '${group}'"
        exit 1
        ;;
esac
