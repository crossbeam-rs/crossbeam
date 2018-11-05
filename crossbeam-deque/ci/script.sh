#!/bin/bash

set -ex

export RUSTFLAGS="-D warnings"

cargo test
