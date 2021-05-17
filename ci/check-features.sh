#!/bin/bash

cd "$(dirname "$0")"/..
set -ex

if [[ ! -x "$(command -v cargo-hack)" ]]; then
    cargo +stable install --debug cargo-hack || exit 1
fi

if [[ "$RUST_VERSION" != "nightly"* ]]; then
    # On MSRV, features other than nightly should work.
    # * `--feature-powerset` - run for the feature powerset which includes --no-default-features and default features of package
    # * `--no-dev-deps` - build without dev-dependencies to avoid https://github.com/rust-lang/cargo/issues/4866
    # * `--exclude benchmarks` - benchmarks doesn't published.
    # * `--skip nightly` - skip `nightly` feature as requires nightly compilers.
    cargo hack check --all --feature-powerset --no-dev-deps --exclude benchmarks --skip nightly
else
    # On nightly, all feature　combinations should work.
    cargo hack check --all --feature-powerset --no-dev-deps --exclude benchmarks

    # Check for no_std environment.
    rustup target add thumbv7m-none-eabi
    rustup target add thumbv6m-none-eabi
    cargo hack check --all --feature-powerset --no-dev-deps --exclude benchmarks --target thumbv7m-none-eabi --skip std,default
    cargo hack check --all --feature-powerset --no-dev-deps --exclude benchmarks --target thumbv6m-none-eabi --skip std,default
fi
