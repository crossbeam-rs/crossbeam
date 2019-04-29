#!/bin/bash

cd "$(dirname "$0")"/../crossbeam-skiplist
set -ex

export RUSTFLAGS="-D warnings"

cargo check --no-default-features
cargo check --bins --examples --tests
cargo test

if [[ "$TRAVIS_RUST_VERSION" == "nightly" ]]; then
    cargo check --no-default-features --features nightly
    cargo test --features nightly

    # Check for no_std environment.
    cd .. && cargo run --manifest-path ci/Cargo.toml --bin remove-dev-dependencies */Cargo.toml && cd -
    cargo check --target thumbv7m-none-eabi --no-default-features
    cargo check --target thumbv7m-none-eabi --no-default-features --features nightly
fi
