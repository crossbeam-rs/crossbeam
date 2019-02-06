#!/bin/bash

cd "$(dirname "$0")"/..
set -ex

export RUSTFLAGS="-D warnings"

cargo check --all --no-default-features
cargo check --all --bins --examples --tests
cargo test --all

if [[ "$TRAVIS_RUST_VERSION" == "nightly" ]]; then
    cargo check --all --no-default-features --features nightly
    cargo test --all --features nightly
fi
