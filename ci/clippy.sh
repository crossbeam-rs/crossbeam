#!/bin/bash

cd "$(dirname "$0")"/..
set -ex

rustup component add clippy

cargo clippy --all
