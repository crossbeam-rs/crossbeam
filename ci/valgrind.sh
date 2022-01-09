#!/bin/bash

cd "$(dirname "$0")"/..
set -ex

CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER="valgrind --error-exitcode=1 --leak-check=full --show-leak-kinds=all" \
    cargo test --all
