// Translation of the following Go program into Rust.
// Source: https://www.nada.kth.se/~snilsson/concurrency/#Match

/*
func main() {
    people := []string{"Anna", "Bob", "Cody", "Dave", "Eva"}
    match := make(chan string, 1) // Make room for one unmatched send.
    wg := new(sync.WaitGroup)
    for _, name := range people {
        wg.Add(1)
        go Seek(name, match, wg)
    }
    wg.Wait()
    select {
    case name := <-match:
        fmt.Printf("No one received %s’s message.\n", name)
    default:
        // There was no pending send operation.
    }
}

// Seek either sends or receives, whichever possible, a name on the match
// channel and notifies the wait group when done.
func Seek(name string, match chan string, wg *sync.WaitGroup) {
    select {
    case peer := <-match:
        fmt.Printf("%s received a message from %s.\n", name, peer)
    case match <- name:
        // Wait for someone to receive my message.
    }
    wg.Done()
}
*/

#[macro_use]
extern crate channel;
extern crate crossbeam;

use channel::{Receiver, Sender};

fn main() {
    let people = vec!["Anna", "Bob", "Cody", "Dave", "Eva"];
    let (tx, rx) = channel::bounded(1); // Make room for one unmatched send.
    let (tx, rx) = (&tx, &rx);

    crossbeam::scope(|s| {
        for name in people {
            s.spawn(move || seek(name, tx, rx));
        }
    });

    if let Ok(name) = rx.try_recv() {
        println!("No one received {}’s message.", name);
    }
}

// Either sends or receives, whichever possible, a name on the channel.
fn seek<'a>(name: &'a str, tx: &Sender<&'a str>, rx: &Receiver<&'a str>) {
    select_loop! {
        recv(rx, peer) => println!("{} received a message from {}.", name, peer),
        send(tx, name) => {}, // Wait for someone to receive my message.
    }
}
