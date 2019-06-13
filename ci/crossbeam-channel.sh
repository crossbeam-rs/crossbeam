#!/bin/bash

cd "$(dirname "$0")"/../crossbeam-channel
set -ex

if [[ "$TRAVIS_RUST_VERSION" != "nightly" ]]; then
    export RUSTFLAGS="-D warnings"
fi

cargo check --bins --examples --tests
cargo test -- --test-threads=1

if [[ "$TRAVIS_RUST_VERSION" == "nightly" ]]; then
    cd benchmarks
    cargo check --bins
fi
