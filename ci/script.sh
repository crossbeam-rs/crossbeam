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
fi

# TODO(stjepang): Uncomment the following lines once we fix the dependency tree
# if [[ "$TRAVIS_RUST_VERSION" == "nightly" ]]; then
#     cargo install cargo-tree
#     (cargo tree --duplicate | grep "^crossbeam") && exit 1
# fi
