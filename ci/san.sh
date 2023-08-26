#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"/..

if [[ "$OSTYPE" != "linux"* ]]; then
    exit 0
fi

rustup component add rust-src

# Run address sanitizer
# TODO: Once `cfg(sanitize = "..")` is stable, replace
# `cfg(crossbeam_sanitize)` with `cfg(sanitize = "..")` and remove
# `--cfg crossbeam_sanitize`.
cargo clean
RUSTFLAGS="${RUSTFLAGS:-} -Z sanitizer=address --cfg crossbeam_sanitize" \
    cargo test -Z build-std --all --release --target x86_64-unknown-linux-gnu --tests --exclude benchmarks -- --test-threads=1

RUSTFLAGS="${RUSTFLAGS:-} -Z sanitizer=address --cfg crossbeam_sanitize" \
    cargo run -Z build-std \
    --release \
    --target x86_64-unknown-linux-gnu \
    --example sanitize \
    --manifest-path crossbeam-epoch/Cargo.toml

# Run memory sanitizer
cargo clean
RUSTFLAGS="${RUSTFLAGS:-} -Z sanitizer=memory --cfg crossbeam_sanitize" \
    cargo test -Z build-std --all --release --target x86_64-unknown-linux-gnu --tests --exclude benchmarks -- --test-threads=1

# Run thread sanitizer
cargo clean
TSAN_OPTIONS="suppressions=$(pwd)/ci/tsan" \
RUSTFLAGS="${RUSTFLAGS:-} -Z sanitizer=thread --cfg crossbeam_sanitize" \
    cargo test -Z build-std --all --release --target x86_64-unknown-linux-gnu --tests --exclude benchmarks -- --test-threads=1
