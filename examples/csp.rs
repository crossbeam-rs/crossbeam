//! Translation of the following Go program into Rust.
//! Source: https://www.nada.kth.se/~snilsson/concurrency/#Match
//!
//! ```go
//! func main() {
//!     people := []string{"Anna", "Bob", "Cody", "Dave", "Eva"}
//!     match := make(chan string, 1) // Make room for one unmatched send.
//!     wg := new(sync.WaitGroup)
//!     for _, name := range people {
//!         wg.Add(1)
//!         go Seek(name, match, wg)
//!     }
//!     wg.Wait()
//!     select {
//!     case name := <-match:
//!         fmt.Printf("No one received %s’s message.\n", name)
//!     default:
//!         // There was no pending send operation.
//!     }
//! }
//!
//! // Seek either sends or receives, whichever possible, a name on the match
//! // channel and notifies the wait group when done.
//! func Seek(name string, match chan string, wg *sync.WaitGroup) {
//!     select {
//!     case peer := <-match:
//!         fmt.Printf("%s received a message from %s.\n", name, peer)
//!     case match <- name:
//!         // Wait for someone to receive my message.
//!     }
//!     wg.Done()
//! }
//! ```

extern crate crossbeam;
#[macro_use]
extern crate crossbeam_channel as channel;

fn main() {
    let people = vec!["Anna", "Bob", "Cody", "Dave", "Eva"];
    let (s, r) = &channel::bounded(1); // Make room for one unmatched send.

    // Either send my name into the channel or receive someone else's, whatever happens first.
    let seek = |name, s: &channel::Sender<&str>, r: &channel::Receiver<&str>| {
        select! {
            recv(r, peer) => println!("{} received a message from {}.", name, peer.unwrap()),
            send(s, name) => {}, // Wait for someone to receive my message.
        }
    };

    crossbeam::scope(|scope| {
        for name in people {
            scope.spawn(move || seek(name, s, r));
        }
    });

    if let Some(name) = r.try_recv() {
        println!("No one received {}’s message.", name);
    }
}
