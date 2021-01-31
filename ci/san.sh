#!/bin/bash

cd "$(dirname "$0")"/..
set -ex

if [[ "$OSTYPE" != "linux"* ]]; then
    exit 0
fi

rustup component add rust-src

# Run address sanitizer
# https://github.com/crossbeam-rs/crossbeam/issues/614
export ASAN_OPTIONS="detect_leaks=0"
cargo clean
# TODO: Once `cfg(sanitize = "..")` is stable, replace
# `cfg(crossbeam_sanitize)` with `cfg(sanitize = "..")` and remove
# `--cfg crossbeam_sanitize`.
RUSTFLAGS="-Dwarnings -Zsanitizer=address --cfg crossbeam_sanitize" \
cargo test --all --release --target x86_64-unknown-linux-gnu --tests --exclude benchmarks -- --test-threads=1

cargo clean
RUSTFLAGS="-Dwarnings -Zsanitizer=address --cfg crossbeam_sanitize" \
cargo run \
    --release \
    --target x86_64-unknown-linux-gnu \
    --features nightly \
    --example sanitize \
    --manifest-path crossbeam-epoch/Cargo.toml

# Run memory sanitizer
cargo clean
RUSTFLAGS="-Dwarnings -Zsanitizer=memory --cfg crossbeam_sanitize" \
cargo test -Zbuild-std --all --release --target x86_64-unknown-linux-gnu --tests --exclude benchmarks -- --test-threads=1

# Run thread sanitizer
export TSAN_OPTIONS="suppressions=$(pwd)/ci/tsan"
cargo clean
RUSTFLAGS="-Dwarnings -Zsanitizer=thread --cfg crossbeam_sanitize" \
cargo test -Zbuild-std --all --release --target x86_64-unknown-linux-gnu --tests --exclude benchmarks -- --test-threads=1
