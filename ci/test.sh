#!/bin/bash

cd "$(dirname "$0")"/..
set -ex

export RUSTFLAGS="-D warnings"

if [[ -n "$TARGET" ]]; then
    # If TARGET is specified, use cross for testing.
    cargo install cross
    cross test --all --target "$TARGET" --exclude benchmarks

    # For now, the non-host target only runs tests.
    exit 0
fi

# Otherwise, run tests and checks with the host target.
cargo check --all --bins --examples --tests --exclude benchmarks
cargo test --all --exclude benchmarks -- --test-threads=1

if [[ "$RUST_VERSION" == "nightly"* ]]; then
    # Some crates have `nightly` feature, so run tests with --all-features.
    cargo test --all --all-features --exclude benchmarks -- --test-threads=1

    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all --all-features

    # Benchmarks are only checked on nightly because depending on unstable features.
    cargo check --all --benches
    cd crossbeam-channel/benchmarks
    cargo check --bins
    cd ..

    # Run address sanitizer on crossbeam-epoch
    # Note: this will be significantly rewritten by https://github.com/crossbeam-rs/crossbeam/pull/591.
    if [[ "$OSTYPE" == "linux"* ]]; then
        cd crossbeam-epoch
        cargo clean

        ASAN_OPTIONS="detect_odr_violation=0 detect_leaks=0" \
        RUSTFLAGS="-Z sanitizer=address" \
        cargo run \
            --release \
            --target x86_64-unknown-linux-gnu \
            --features sanitize,nightly \
            --example sanitize

        cd ..
    fi
fi
