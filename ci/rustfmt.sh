#!/bin/bash

cd "$(dirname "$0")"/..
set -ex

cargo fmt --all -- --check
