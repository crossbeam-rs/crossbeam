#[macro_use]
extern crate crossbeam_channel as chan;

fn main() {
    select! {} //~ ERROR empty block in `select!`
}
