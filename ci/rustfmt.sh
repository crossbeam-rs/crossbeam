#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"/..

rustup component add rustfmt

cargo fmt --all -- --check
