#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"/..

export RUSTDOCFLAGS="-D warnings"

cargo doc --no-deps --all --all-features
