#!/bin/bash

cd "$(dirname "$0")"/..
set -ex

export RUSTFLAGS="-D warnings"

if [[ -n "$TARGET" ]]; then
    # If TARGET is specified, use cross for testing.
    cargo install cross
    cross test --all --target "$TARGET" --exclude benchmarks -- --test-threads=1

    # For now, the non-host target only runs tests.
    exit 0
fi

# Otherwise, run tests and checks with the host target.
cargo check --all --bins --examples --tests --exclude benchmarks
cargo test --all --exclude benchmarks -- --test-threads=1

if [[ "$RUST_VERSION" == "nightly"* ]]; then
    # Some crates have `nightly` feature, so run tests with --all-features.
    cargo test --all --all-features --exclude benchmarks -- --test-threads=1

    # Benchmarks are only checked on nightly because depending on unstable features.
    cargo check --all --benches
    cargo check --bins --manifest-path crossbeam-channel/benchmarks/Cargo.toml
fi
