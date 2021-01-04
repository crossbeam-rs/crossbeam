#!/bin/bash

cd "$(dirname "$0")"/..
set -ex

export RUSTDOCFLAGS="-D warnings"

cargo doc --no-deps --all --all-features
