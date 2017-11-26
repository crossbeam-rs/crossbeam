# Benchmarks

### Tests

* `seq`: A single thread sends `N` messages. Then it receives `N` messages.
* `spsc`: One thread sends `N` messages. Another thread receives `N` messages.
* `spsc`: One thread sends `N` messages. Another thread receives `N` messages.
* `mpsc`: `T` threads send `N / T` messages each. One thread receives `N` messages.
* `mpmc`: `T` threads send `N / T` messages each. `T` other threads receive `N / T` messages each.
* `select_rx`: `T` threads send `N / T` messages each into a separate channel. Another thread receives `N` messages by selecting over the `T` channels.
* `select_both`: `T` threads send `N / T` messages each by selecting over `T` channels. `T` other threads receive `N / T` messages each by selecting over the `T` channels.

### Running

```
cargo run --release --bin chan | tee chan.txt
cargo run --release --bin channel | tee channel.txt
cargo run --release --bin mpsc | tee mpsc.txt
cargo run --release --bin ms_queue | tee ms_queue.txt
cargo run --release --bin seg_queue | tee seg_queue.txt
go run main.go | tee go.txt
./plot.py *.txt
```

### Results

Benchmarked on 2017-11-09:

![Benchmark results](https://i.imgur.com/W0cSEVd.png)
