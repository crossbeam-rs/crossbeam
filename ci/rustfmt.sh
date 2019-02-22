#!/bin/bash

cd "$(dirname "$0")"/..
set -ex

if [[ ! -x "$(command -v rustfmt)" ]]; then
    cargo install --debug rustfmt || exit 1
fi

cargo fmt --all -- --check
