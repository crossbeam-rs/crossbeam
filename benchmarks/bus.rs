extern crate crossbeam;
extern crate bus;

use bus::*;

const MESSAGES: usize = 5_000_000;
const THREADS: usize = 4;

fn seq(cap: usize) {
    let mut tx = Bus::new(cap);
    let mut rx = tx.add_rx();

    for i in 0..MESSAGES {
        tx.broadcast(i as i32);
    }

    for _ in 0..MESSAGES {
        rx.recv().unwrap();
    }
}

fn spsc(cap: usize) {
    let mut tx = Bus::new(cap);
    let mut rx = tx.add_rx();

    crossbeam::scope(|s| {
        s.spawn(|| {
            for i in 0..MESSAGES {
                tx.broadcast(i as i32);
            }
        });

        for _ in 0..MESSAGES {
            rx.recv().unwrap();
        }
    });
}

// fn mpsc<F: Fn() -> TxRx>(make: F) {
//     let (tx, rx) = make();
//
//     crossbeam::scope(|s| {
//         for _ in 0..THREADS {
//             s.spawn(|| {
//                 for i in 0..MESSAGES / THREADS {
//                     tx.send(i as i32).unwrap();
//                 }
//             });
//         }
//         s.spawn(|| {
//             for _ in 0..MESSAGES {
//                 rx.recv().unwrap();
//             }
//         });
//     });
// }
//
// fn mpmc<F: Fn() -> TxRx>(make: F) {
//     let (tx, rx) = make();
//
//     crossbeam::scope(|s| {
//         for _ in 0..THREADS {
//             s.spawn(|| {
//                 for i in 0..MESSAGES / THREADS {
//                     tx.send(i as i32).unwrap();
//                 }
//             });
//         }
//         for _ in 0..THREADS {
//             s.spawn(|| {
//                 for _ in 0..MESSAGES / THREADS {
//                     rx.recv().unwrap();
//                 }
//             });
//         }
//     });
// }
//
// fn select_rx<F: Fn() -> TxRx>(make: F) {
//     let chans = (0..THREADS).map(|_| make()).collect::<Vec<_>>();
//
//     crossbeam::scope(|s| {
//         for &(ref tx, _) in &chans {
//             s.spawn(move || {
//                 for i in 0..MESSAGES / THREADS {
//                     tx.send(i as i32).unwrap();
//                 }
//             });
//         }
//
//         s.spawn(|| {
//             for _ in 0..MESSAGES {
//                 let mut sel = Select::new();
//                 'select: loop {
//                     for &(_, ref rx) in &chans {
//                         if let Ok(_) = sel.recv(&rx) {
//                             break 'select;
//                         }
//                     }
//                 }
//             }
//         });
//     });
// }
//
// fn select_both<F: Fn() -> TxRx>(make: F) {
//     let chans = (0..THREADS).map(|_| make()).collect::<Vec<_>>();
//
//     crossbeam::scope(|s| {
//         for _ in 0..THREADS {
//             s.spawn(|| {
//                 for i in 0..MESSAGES / THREADS {
//                     let mut sel = Select::new();
//                     'select: loop {
//                         for &(ref tx, _) in &chans {
//                             if let Ok(_) = sel.send(&tx, i as i32) {
//                                 break 'select;
//                             }
//                         }
//                     }
//                 }
//             });
//         }
//
//         for _ in 0..THREADS {
//             s.spawn(|| {
//                 for _ in 0..MESSAGES / THREADS {
//                     let mut sel = Select::new();
//                     'select: loop {
//                         for &(_, ref rx) in &chans {
//                             if let Ok(_) = sel.recv(&rx) {
//                                 break 'select;
//                             }
//                         }
//                     }
//                 }
//             });
//         }
//     });
// }

fn main() {
    macro_rules! run {
        ($name:expr, $f:expr) => {
            let now = ::std::time::Instant::now();
            $f;
            let elapsed = now.elapsed();
            println!(
                "{:25} {:15} {:7.3} sec",
                $name,
                "Rust bus",
                elapsed.as_secs() as f64 + elapsed.subsec_nanos() as f64 / 1e9
            );
        }
    }

    // run!("bounded1_mpmc", mpmc(|| bounded(1)));
    // run!("bounded1_mpsc", mpsc(|| bounded(1)));
    // run!("bounded1_select_both", select_both(|| bounded(1)));
    // run!("bounded1_select_rx", select_rx(|| bounded(1)));
    run!("bounded1_spsc", spsc(1));

    // run!("bounded_mpmc", mpmc(|| bounded(MESSAGES)));
    // run!("bounded_mpsc", mpsc(|| bounded(MESSAGES)));
    // run!("bounded_select_both", select_both(|| bounded(MESSAGES)));
    // run!("bounded_select_rx", select_rx(|| bounded(MESSAGES)));
    run!("bounded_seq", seq(MESSAGES));
    run!("bounded_spsc", spsc(MESSAGES));
}
