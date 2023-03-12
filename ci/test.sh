#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"/..

# shellcheck disable=SC2086
if [[ -n "${RUST_TARGET:-}" ]]; then
    cargo test --all --target "$RUST_TARGET" --exclude benchmarks ${DOCTEST_XCOMPILE:-} -- --test-threads=1
    cargo test --all --target "$RUST_TARGET" --exclude benchmarks --release ${DOCTEST_XCOMPILE:-} -- --test-threads=1

    # For now, the non-host target only runs tests.
    exit 0
fi

# Otherwise, run tests and checks with the host target.
cargo test --all --all-features --exclude benchmarks -- --test-threads=1
cargo test --all --all-features --exclude benchmarks --release -- --test-threads=1

if [[ "$RUST_VERSION" == "nightly"* ]]; then
    # Benchmarks are only checked on nightly because depending on unstable features.
    cargo check --all --all-targets
fi
