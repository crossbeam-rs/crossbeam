#!/bin/bash

cd "$(dirname "$0")"/../crossbeam-queue
set -ex

export RUSTFLAGS="-D warnings"

cargo check --no-default-features
cargo check --bins --examples --tests
cargo test
