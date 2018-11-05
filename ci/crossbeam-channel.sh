#!/bin/bash

cd "$(dirname "$0")"/../crossbeam-channel
set -ex

export RUSTFLAGS="-D warnings"

cargo test -- --test-threads=1

if [[ "$TRAVIS_RUST_VERSION" == "nightly" ]]; then
    cd benchmarks
    cargo build --bins
fi
