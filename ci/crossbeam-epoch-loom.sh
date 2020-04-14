#!/bin/bash

cd "$(dirname "$0")"/../crossbeam-epoch
set -ex

export RUSTFLAGS="-D warnings --cfg=loom"

env LOOM_MAX_PREEMPTIONS=2 cargo test --test loom --features sanitize --release -- --nocapture
