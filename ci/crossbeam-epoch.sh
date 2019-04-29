#!/bin/bash

cd "$(dirname "$0")"/../crossbeam-epoch
set -ex

export RUSTFLAGS="-D warnings"

cargo check --no-default-features
cargo check --bins --examples --tests
cargo test

if [[ "$TRAVIS_RUST_VERSION" == "nightly" ]]; then
    cargo check --no-default-features --features nightly
    cargo test --features nightly

    ASAN_OPTIONS="detect_odr_violation=0 detect_leaks=0" \
    RUSTFLAGS="-Z sanitizer=address" \
    cargo run \
        --release \
        --target x86_64-unknown-linux-gnu \
        --features sanitize,nightly \
        --example sanitize

    # Check for no_std environment.
    cd .. && cargo run --manifest-path ci/Cargo.toml --bin remove-dev-dependencies */Cargo.toml && cd -
    cargo check --target thumbv7m-none-eabi --no-default-features
    cargo check --target thumbv7m-none-eabi --no-default-features --features nightly
fi
