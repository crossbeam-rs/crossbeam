#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"/../crossbeam-epoch

export RUSTFLAGS="${RUSTFLAGS:-} --cfg crossbeam_loom --cfg crossbeam_sanitize"

# With MAX_PREEMPTIONS=2 the loom tests (currently) take around 11m.
# If we were to run with =3, they would take several times that,
# which is probably too costly for CI.
env LOOM_MAX_PREEMPTIONS=2 cargo test --test loom --release --features loom -- --nocapture
