#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"/..

# * `--feature-powerset` - run for the feature powerset which includes --no-default-features and default features of package
# * `--no-dev-deps` - build without dev-dependencies to avoid https://github.com/rust-lang/cargo/issues/4866
# * `--exclude benchmarks` - benchmarks doesn't published.
if [[ "${RUST_VERSION}" == "msrv" ]]; then
    cargo hack build --all --feature-powerset --no-dev-deps --exclude crossbeam-utils --exclude benchmarks --rust-version
    # atomic feature requires Rust 1.60.
    cargo hack build -p crossbeam-utils --feature-powerset --no-dev-deps --rust-version --exclude-features atomic
    cargo +1.60 hack build -p crossbeam-utils --feature-powerset --no-dev-deps
else
    cargo hack build --all --feature-powerset --no-dev-deps --exclude benchmarks
fi

if [[ "${RUST_VERSION}" == "nightly"* ]]; then
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
