#!/bin/bash

set -ex

cargo build
cargo build --no-default-features
cargo test

if [[ $TRAVIS_RUST_VERSION == nightly ]]; then
    cargo test --features nightly
fi
