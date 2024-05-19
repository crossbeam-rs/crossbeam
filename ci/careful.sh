#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"/..

# TODO: Use the system's default linker instead of rust-lld, which recently
# became the default until this cargo-careful bug is fixed:
#   https://github.com/RalfJung/cargo-careful/issues/31
export RUSTFLAGS="${RUSTFLAGS:-} -Z randomize-layout -Z linker-features=-lld"

cargo careful test --all --all-features --exclude benchmarks -- --test-threads=1
