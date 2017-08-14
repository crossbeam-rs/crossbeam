# Benchmarks

These benchmarks compare performance of several channel implementations:

* this crate
* `std::sync::mpsc`
* [chan](https://docs.rs/chan)
* [multiqueue](https://docs.rs/multiqueue)
* Go channels

### Running

```
cargo run --release --bin chan | tee chan.txt
cargo run --release --bin channel | tee channel.txt
cargo run --release --bin mpsc | tee mpsc.txt
cargo run --release --bin multiqueue | tee multiqueue.txt
go run main.go | tee go.txt
./format.py *.txt
```

### Results on my machine

```
bounded0_mpmc             Go chan           2.020 sec
bounded0_mpmc             Rust chan        85.972 sec
bounded0_mpmc             Rust channel      2.308 sec

bounded0_mpsc             Go chan           1.744 sec
bounded0_mpsc             Rust chan        44.248 sec
bounded0_mpsc             Rust channel      2.335 sec
bounded0_mpsc             Rust mpsc        27.391 sec

bounded0_select_both      Go chan           5.381 sec
bounded0_select_both      Rust channel     13.026 sec

bounded0_select_rx        Go chan           4.086 sec
bounded0_select_rx        Rust chan        11.624 sec
bounded0_select_rx        Rust channel      3.160 sec
bounded0_select_rx        Rust mpsc        11.402 sec

bounded0_spsc             Go chan           1.334 sec
bounded0_spsc             Rust chan        20.746 sec
bounded0_spsc             Rust channel      2.315 sec
bounded0_spsc             Rust mpsc        25.145 sec

bounded1_mpmc             Go chan           1.792 sec
bounded1_mpmc             Rust chan        20.661 sec
bounded1_mpmc             Rust channel      0.564 sec
bounded1_mpmc             Rust multiqueue   2.269 sec

bounded1_mpsc             Go chan           1.417 sec
bounded1_mpsc             Rust chan        16.558 sec
bounded1_mpsc             Rust channel      0.539 sec
bounded1_mpsc             Rust mpsc        27.858 sec
bounded1_mpsc             Rust multiqueue   2.305 sec

bounded1_select_both      Go chan           4.268 sec
bounded1_select_both      Rust chan        12.962 sec
bounded1_select_both      Rust channel      5.924 sec

bounded1_select_rx        Go chan           3.591 sec
bounded1_select_rx        Rust chan        11.545 sec
bounded1_select_rx        Rust channel      3.448 sec
bounded1_select_rx        Rust mpsc        11.161 sec

bounded1_spsc             Go chan           1.177 sec
bounded1_spsc             Rust chan        15.436 sec
bounded1_spsc             Rust channel      0.949 sec
bounded1_spsc             Rust mpsc        26.012 sec
bounded1_spsc             Rust multiqueue   1.266 sec

bounded_mpmc              Go chan           0.821 sec
bounded_mpmc              Rust chan         2.726 sec
bounded_mpmc              Rust channel      0.249 sec

bounded_mpsc              Go chan           0.672 sec
bounded_mpsc              Rust chan         1.845 sec
bounded_mpsc              Rust channel      0.258 sec
bounded_mpsc              Rust mpsc         1.418 sec
bounded_mpsc              Rust multiqueue   0.515 sec

bounded_select_both       Go chan           1.838 sec
bounded_select_both       Rust chan         4.178 sec
bounded_select_both       Rust channel      0.850 sec

bounded_select_rx         Go chan           1.670 sec
bounded_select_rx         Rust chan         3.844 sec
bounded_select_rx         Rust channel      0.932 sec
bounded_select_rx         Rust mpsc         1.096 sec

bounded_seq               Go chan           0.318 sec
bounded_seq               Rust chan         0.900 sec
bounded_seq               Rust channel      0.189 sec
bounded_seq               Rust mpsc         0.505 sec
bounded_seq               Rust multiqueue   0.326 sec

bounded_spsc              Go chan           0.648 sec
bounded_spsc              Rust chan         2.847 sec
bounded_spsc              Rust channel      0.137 sec
bounded_spsc              Rust mpsc         2.008 sec
bounded_spsc              Rust multiqueue   0.622 sec

unbounded_mpmc            Rust chan         2.664 sec
unbounded_mpmc            Rust channel      0.637 sec

unbounded_mpsc            Rust chan         1.775 sec
unbounded_mpsc            Rust channel      0.588 sec
unbounded_mpsc            Rust mpsc         0.489 sec

unbounded_select_both     Rust chan         4.229 sec
unbounded_select_both     Rust channel      1.072 sec

unbounded_select_rx       Rust chan         3.855 sec
unbounded_select_rx       Rust channel      1.181 sec
unbounded_select_rx       Rust mpsc         0.477 sec

unbounded_seq             Rust chan         0.793 sec
unbounded_seq             Rust channel      0.564 sec
unbounded_seq             Rust mpsc         0.549 sec

unbounded_spsc            Rust chan         2.420 sec
unbounded_spsc            Rust channel      0.586 sec
unbounded_spsc            Rust mpsc         0.833 sec
```
