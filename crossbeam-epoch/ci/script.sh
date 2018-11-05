#!/bin/bash

check_min_rustc() {
    local rustc="`rustc -V | cut -d' ' -f2 | cut -d- -f1`"
    if [[ "$rustc" != "`echo -e "$rustc\n$1" | sort -V | tail -n1`" ]]; then
        echo "Unsupported Rust version: $rustc < $1"
        exit 0
    fi
}
check_min_rustc 1.26.0

set -ex

export RUSTFLAGS="-D warnings"

cargo build --no-default-features
cargo test

if [[ "$TRAVIS_RUST_VERSION" == "nightly" ]]; then
    cargo test --features nightly

    sudo apt-get install -y llvm-3.8 llvm-3.8-dev clang-3.8 clang-3.8-dev

    ASAN_OPTIONS="detect_odr_violation=0 detect_leaks=0" \
    RUSTFLAGS="-Z sanitizer=address" \
    cargo run \
        --release \
        --target x86_64-unknown-linux-gnu \
        --features sanitize,nightly \
        --example sanitize
fi
