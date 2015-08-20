/*

mem - memory management
    epoch
    cache_padded
sync -
    atomic_option
    msqueue
scope
parallel

*/

#![feature(test)]
#![feature(duration_span)]
#![feature(fnbox)]
#![feature(box_patterns)]
#![feature(box_raw)]
#![feature(const_fn)]
#![feature(optin_builtin_traits)]
#![feature(drain)]

pub mod atomic_option;
pub mod mem;
pub mod queue;
pub mod thread;

mod cache_padded;
