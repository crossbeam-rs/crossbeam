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

cargo test -- --test-threads=1

if [[ "$TRAVIS_RUST_VERSION" == "nightly" ]]; then
    cd benchmarks
    cargo build --bins
fi
