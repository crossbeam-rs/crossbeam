#!/bin/bash

cd "$(dirname "$0")"/..
set -ex

if [[ ! -x "$(command -v cargo-tree)" ]]; then
    cargo install --debug cargo-tree || exit 1
fi

cargo tree
cargo tree --duplicate
cargo tree --duplicate || exit 1

# Check minimal versions.
cargo update -Zminimal-versions
cargo tree
cargo check --all --all-features --exclude benchmarks

exit 0
