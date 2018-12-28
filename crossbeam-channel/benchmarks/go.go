package main

import "fmt"
import "time"

const MESSAGES = 5 * 1000 * 1000
const THREADS = 4

type Message = uintptr

func seq(cap int) {
    var c = make(chan Message, cap)

    for i := 0; i < MESSAGES; i++ {
        c <- Message(i)
    }

    for i := 0; i < MESSAGES; i++ {
        <-c
    }
}

func spsc(cap int) {
    var c = make(chan Message, cap)
    var done = make(chan bool)

    go func() {
        for i := 0; i < MESSAGES; i++ {
            c <- Message(i)
        }
        done <- true
    }()

    for i := 0; i < MESSAGES; i++ {
        <-c
    }

    <-done
}

func mpsc(cap int) {
    var c = make(chan Message, cap)
    var done = make(chan bool)

    for t := 0; t < THREADS; t++ {
        go func() {
            for i := 0; i < MESSAGES / THREADS; i++ {
                c <- Message(i)
            }
            done <- true
        }()
    }

    for i := 0; i < MESSAGES; i++ {
        <-c
    }

    for t := 0; t < THREADS; t++ {
        <-done
    }
}

func mpmc(cap int) {
    var c = make(chan Message, cap)
    var done = make(chan bool)

    for t := 0; t < THREADS; t++ {
        go func() {
            for i := 0; i < MESSAGES / THREADS; i++ {
                c <- Message(i)
            }
            done <- true
        }()

    }

    for t := 0; t < THREADS; t++ {
        go func() {
            for i := 0; i < MESSAGES / THREADS; i++ {
                <-c
            }
            done <- true
        }()
    }

    for t := 0; t < THREADS; t++ {
        <-done
        <-done
    }
}

func select_rx(cap int) {
    if THREADS != 4 {
        panic("assumed there are 4 threads")
    }

    var c0 = make(chan Message, cap)
    var c1 = make(chan Message, cap)
    var c2 = make(chan Message, cap)
    var c3 = make(chan Message, cap)
    var done = make(chan bool)

    var producer = func(c chan Message) {
        for i := 0; i < MESSAGES / THREADS; i++ {
            c <- Message(i)
        }
        done <- true
    }
    go producer(c0)
    go producer(c1)
    go producer(c2)
    go producer(c3)

    for i := 0; i < MESSAGES; i++ {
        select {
        case <-c0:
        case <-c1:
        case <-c2:
        case <-c3:
        }
    }

    for t := 0; t < THREADS; t++ {
        <-done
    }
}

func select_both(cap int) {
    if THREADS != 4 {
        panic("assumed there are 4 threads")
    }

    var c0 = make(chan Message, cap)
    var c1 = make(chan Message, cap)
    var c2 = make(chan Message, cap)
    var c3 = make(chan Message, cap)
    var done = make(chan bool)

    var producer = func(c chan Message) {
        for i := 0; i < MESSAGES / THREADS; i++ {
            c <- Message(i)
        }
        done <- true
    }
    go producer(c0)
    go producer(c1)
    go producer(c2)
    go producer(c3)

    for t := 0; t < THREADS; t++ {
        go func() {
            for i := 0; i < MESSAGES / THREADS; i++ {
                select {
                case <-c0:
                case <-c1:
                case <-c2:
                case <-c3:
                }
            }
            done <- true
        }()
    }

    for t := 0; t < THREADS; t++ {
        <-done
        <-done
    }
}

func run(name string, f func(int), cap int) {
    var now = time.Now()
    f(cap)
    var elapsed = time.Now().Sub(now)
    fmt.Printf("%-25v %-15v %7.3f sec\n", name, "Go chan", float64(elapsed) / float64(time.Second))
}

func main() {
    run("bounded0_mpmc", mpmc, 0)
    run("bounded0_mpsc", mpsc, 0)
    run("bounded0_select_both", select_both, 0)
    run("bounded0_select_rx", select_rx, 0)
    run("bounded0_spsc", spsc, 0)

    run("bounded1_mpmc", mpmc, 1)
    run("bounded1_mpsc", mpsc, 1)
    run("bounded1_select_both", select_both, 1)
    run("bounded1_select_rx", select_rx, 1)
    run("bounded1_spsc", spsc, 1)

    run("bounded_mpmc", mpmc, MESSAGES)
    run("bounded_mpsc", mpsc, MESSAGES)
    run("bounded_select_both", select_both, MESSAGES)
    run("bounded_select_rx", select_rx, MESSAGES)
    run("bounded_seq", seq, MESSAGES)
    run("bounded_spsc", spsc, MESSAGES)
}
