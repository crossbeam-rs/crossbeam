#!/bin/bash

cd "$(dirname "$0")"/../crossbeam-utils
set -ex

export RUSTFLAGS="-D warnings"

cargo check --no-default-features
cargo check --bins --examples --tests
cargo test

if [[ "$RUST_VERSION" == "nightly" ]]; then
    cargo check --no-default-features --features nightly
    cargo test --features nightly

    # Check for no_std environment.
    cargo check --target thumbv7m-none-eabi --no-default-features
    cargo check --target thumbv7m-none-eabi --no-default-features --features alloc
    cargo check --target thumbv7m-none-eabi --no-default-features --features alloc,nightly
    cargo check --target thumbv6m-none-eabi --no-default-features --features nightly
    cargo check --target thumbv6m-none-eabi --no-default-features --features alloc,nightly
fi
