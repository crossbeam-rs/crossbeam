#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"/..

if [[ "$RUST_VERSION" != "nightly"* ]]; then
    # On MSRV, features other than nightly should work.
    # * `--feature-powerset` - run for the feature powerset which includes --no-default-features and default features of package
    # * `--no-dev-deps` - build without dev-dependencies to avoid https://github.com/rust-lang/cargo/issues/4866
    # * `--exclude benchmarks` - benchmarks doesn't published.
    # * `--skip nightly` - skip `nightly` feature as requires nightly compilers.
    cargo hack build --all --feature-powerset --no-dev-deps --exclude benchmarks --skip nightly
else
    # On nightly, all feature　combinations should work.
    cargo hack build --all --feature-powerset --no-dev-deps --exclude benchmarks

    # Build for no_std environment.
    # thumbv7m-none-eabi supports atomic CAS.
    # thumbv6m-none-eabi supports atomic, but not atomic CAS.
    # riscv32i-unknown-none-elf does not support atomic at all.
    rustup target add thumbv7m-none-eabi
    rustup target add thumbv6m-none-eabi
    rustup target add riscv32i-unknown-none-elf
    cargo hack build --all --feature-powerset --no-dev-deps --exclude benchmarks --target thumbv7m-none-eabi --skip std,default
    cargo hack build --all --feature-powerset --no-dev-deps --exclude benchmarks --target thumbv6m-none-eabi --skip std,default
    cargo hack build --all --feature-powerset --no-dev-deps --exclude benchmarks --target riscv32i-unknown-none-elf --skip std,default
fi
