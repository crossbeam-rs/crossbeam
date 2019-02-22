#!/bin/bash

cd "$(dirname "$0")"/..
set -ex

rustup component add rustfmt

cargo fmt --all -- --check
