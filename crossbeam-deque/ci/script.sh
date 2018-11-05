#!/bin/bash

cd "$(dirname "$0")"/..
set -ex

export RUSTFLAGS="-D warnings"

cargo test
