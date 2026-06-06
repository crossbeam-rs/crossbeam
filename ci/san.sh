#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"/..

if [[ "${OSTYPE}" != "linux"* ]]; then
  exit 0
fi

export RUSTFLAGS="${RUSTFLAGS:-} --cfg crossbeam_sanitize"

# Run address sanitizer
# TODO: Once `cfg(sanitize = "..")` is stable, replace
# `cfg(crossbeam_sanitize)` with `cfg(sanitize = "..")` and remove
# `--cfg crossbeam_sanitize`.
cargo clean
ASAN_OPTIONS="${ASAN_OPTIONS:-} detect_stack_use_after_return=1" \
  cargo test --all --all-features --release --target x86_64-unknown-linux-gnuasan --tests --exclude benchmarks -- --test-threads=1

ASAN_OPTIONS="${ASAN_OPTIONS:-} detect_stack_use_after_return=1" \
  cargo run \
  --all-features \
  --release \
  --target x86_64-unknown-linux-gnuasan \
  --example sanitize \
  --manifest-path crossbeam-epoch/Cargo.toml

# Run memory sanitizer
cargo clean
cargo test -Z build-std --all --all-features --release --target x86_64-unknown-linux-gnumsan --tests --exclude benchmarks -- --test-threads=1

# Run thread sanitizer
cargo clean
TSAN_OPTIONS="${TSAN_OPTIONS:-} suppressions=$(pwd)/ci/tsan" \
  cargo test -Z build-std --all --all-features --release --target x86_64-unknown-linux-gnutsan --tests --exclude benchmarks -- --test-threads=1
