#!/bin/bash

cd "$(dirname "$0")"/..
set -ex

cargo tree
cargo tree --duplicate
cargo tree --duplicate || exit 1

# Check minimal versions.
cargo update -Zminimal-versions
cargo tree
cargo check --all --all-features --exclude benchmarks

exit 0
