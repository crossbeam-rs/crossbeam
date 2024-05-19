#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"/..

export RUSTFLAGS="${RUSTFLAGS:-} -Z randomize-layout"

cargo careful test --all --all-features --exclude benchmarks -- --test-threads=1
