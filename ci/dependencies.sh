#!/bin/bash

cd "$(dirname "$0")"/..
set -ex

if [[ ! -x "$(command -v cargo-tree)" ]]; then
    cargo install --debug cargo-tree || exit 1
fi

(cargo tree --duplicate | grep "^crossbeam") && exit 1
exit 0
