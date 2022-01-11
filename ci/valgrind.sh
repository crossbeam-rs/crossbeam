#!/bin/bash

cd "$(dirname "$0")"/..
set -ex

export RUSTFLAGS="-D warnings --cfg reduce_test_iterations"

CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER="valgrind --error-exitcode=1 --leak-check=full --show-leak-kinds=all" \
    cargo test --all --no-fail-fast -- --test-threads=1
