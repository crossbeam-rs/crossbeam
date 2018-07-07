# Benchmarks

### Tests

* `seq`: A single thread sends `N` messages. Then it receives `N` messages.
* `spsc`: One thread sends `N` messages. Another thread receives `N` messages.
* `mpsc`: `T` threads send `N / T` messages each. One thread receives `N` messages.
* `mpmc`: `T` threads send `N / T` messages each. `T` other threads receive `N / T` messages each.
* `select_rx`: `T` threads send `N / T` messages each into a separate channel. Another thread receives `N` messages by selecting over the `T` channels.
* `select_both`: `T` threads send `N / T` messages each by selecting over `T` channels. `T` other threads receive `N / T` messages each by selecting over the `T` channels.

Default configuration:

- `N = 5000000`
- `T = 4`

### Running

Runs benchmarks, stores results into `*.txt` files, and generates `plot.png`:

```
./run_all.sh
```

Dependencies:

- Rust (nightly)
- Go
- Bash
- Python 2
- Matplotlib

### Results

Machine: Intel(R) Core(TM) i7-5600U (2 physical cores, 4 logical cores)

Benchmarked on 2018-06-11:

![Benchmark results](https://i.imgur.com/tRI4HMO.png)
