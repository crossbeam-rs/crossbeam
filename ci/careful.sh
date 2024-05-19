#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"/..

# TODO: Once cargo-careful's bug (https://github.com/RalfJung/cargo-careful/issues/31) is fixed,
# stop reverting back to the system's default linker, instead of rust-lld, which became the new 
# default on linux recently (nightly-2024-05-18 and onwards).
export RUSTFLAGS="${RUSTFLAGS:-} -Z randomize-layout -Z linker-features=-lld"

cargo careful test --all --all-features --exclude benchmarks -- --test-threads=1
