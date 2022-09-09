#!/bin/bash
set -euxo pipefail
IFS=$'\n\t'
cd "$(dirname "$0")"

cargo run --release --bin crossbeam-channel | tee crossbeam-channel.txt
cargo run --release --bin futures-channel | tee futures-channel.txt
cargo run --release --bin mpsc | tee mpsc.txt
cargo run --release --bin flume | tee flume.txt
go run go.go | tee go.txt

# These can also be run, but too many plot bars mess
# up the plot (they start to overlap). So only 5 contenders
# with the most tests are included by default.

# cargo run --release --bin atomicringqueue | tee atomicringqueue.txt
# cargo run --release --bin atomicring | tee atomicring.txt
# cargo run --release --bin bus | tee bus.txt
# cargo run --release --bin crossbeam-deque | tee crossbeam-deque.txt
# cargo run --release --bin lockfree | tee lockfree.txt
# cargo run --release --bin segqueue | tee segqueue.txt
# cargo run --release --bin mpmc | tee mpmc.txt

./plot.py ./*.txt
