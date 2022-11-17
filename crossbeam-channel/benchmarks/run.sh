#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"

cargo run --release --bin chan | tee chan.txt
cargo run --release --bin crossbeam-channel | tee crossbeam-channel.txt
cargo run --release --bin futures-channel | tee futures-channel.txt
cargo run --release --bin mpsc | tee mpsc.txt
go run go.go | tee go.txt

if ! command -v poetry
then
    echo "poetry (https://python-poetry.org) is required, exiting..."
    exit 1
fi

poetry install
poetry run python plot.py ./*.txt
