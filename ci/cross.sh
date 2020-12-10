#!/bin/bash

cd "$(dirname "$0")"/..
set -ex

cargo install cross

cross test --target "$TARGET" --all --exclude benchmarks
