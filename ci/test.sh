#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"/..

if [[ -n "${RUST_TARGET:-}" ]]; then
    # If RUST_TARGET is specified, use cross for testing.
    cross test --all --target "$RUST_TARGET" --exclude benchmarks -- --test-threads=1

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
