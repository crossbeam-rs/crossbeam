#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"/..

# We need 'ts' for the per-line timing
sudo apt-get -y install moreutils
echo

export RUSTFLAGS="${RUSTFLAGS:-} -Z randomize-layout"
export RUSTDOCFLAGS="${RUSTDOCFLAGS:-} -Z randomize-layout"

MIRIFLAGS="-Zmiri-strict-provenance -Zmiri-symbolic-alignment-check -Zmiri-disable-isolation" \
    cargo miri test \
    -p crossbeam-queue \
    -p crossbeam-utils 2>&1 | ts -i '%.s  '

# -Zmiri-ignore-leaks is needed because we use detached threads in tests/docs: https://github.com/rust-lang/miri/issues/1371
MIRIFLAGS="-Zmiri-strict-provenance -Zmiri-symbolic-alignment-check -Zmiri-disable-isolation -Zmiri-ignore-leaks" \
    cargo miri test \
    -p crossbeam-channel 2>&1 | ts -i '%.s  '

# Use Tree Borrows instead of Stacked Borrows because epoch is not compatible with Stacked Borrows: https://github.com/crossbeam-rs/crossbeam/issues/545#issuecomment-1192785003
MIRIFLAGS="-Zmiri-strict-provenance -Zmiri-symbolic-alignment-check -Zmiri-disable-isolation -Zmiri-tree-borrows" \
    cargo miri test \
    -p crossbeam-epoch \
    -p crossbeam-skiplist \
    -p crossbeam 2>&1 | ts -i '%.s  '

# Use Tree Borrows instead of Stacked Borrows because epoch is not compatible with Stacked Borrows: https://github.com/crossbeam-rs/crossbeam/issues/545#issuecomment-1192785003
# -Zmiri-compare-exchange-weak-failure-rate=0.0 is needed because some sequential tests (e.g.,
# doctest of Stealer::steal) incorrectly assume that sequential weak CAS will never fail.
# -Zmiri-preemption-rate=0 is needed because this code technically has UB and Miri catches that.
MIRIFLAGS="-Zmiri-strict-provenance -Zmiri-symbolic-alignment-check -Zmiri-disable-isolation -Zmiri-tree-borrows -Zmiri-compare-exchange-weak-failure-rate=0.0 -Zmiri-preemption-rate=0" \
    cargo miri test \
    -p crossbeam-deque 2>&1 | ts -i '%.s  '
