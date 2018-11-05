#!/bin/bash

check_min_rustc() {
    local rustc="`rustc -V | cut -d' ' -f2 | cut -d- -f1`"
    if [[ "$rustc" != "`echo -e "$rustc\n$1" | sort -V | tail -n1`" ]]; then
        echo "Unsupported Rust version: $rustc < $1"
        exit 0
    fi
}
check_min_rustc 1.25.0

set -ex

export RUSTFLAGS="-D warnings"

cargo test
