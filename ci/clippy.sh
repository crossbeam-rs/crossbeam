#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"/..

rustup component add clippy

cargo clippy --all --tests --examples
