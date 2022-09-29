#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"/..

cargo tree
cargo tree --duplicate

# Check minimal versions.
cargo minimal-versions build --workspace --all-features --exclude benchmarks
