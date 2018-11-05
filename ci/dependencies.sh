#!/bin/bash

cd "$(dirname "$0")"/..
set -ex

cargo install --debug cargo-tree
(cargo tree --duplicate | grep "^crossbeam") && exit 1
