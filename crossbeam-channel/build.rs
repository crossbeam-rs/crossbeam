// The rustc-cfg listed below are considered public API, but it is *unstable*
// and outside of the normal semver guarantees:
//
// - `crossbeam_no_atomic_64`
//      Assume the target does *not* support AtomicU64/AtomicI64.
//      This is usually detected automatically by the build script, but you may
//      need to enable it manually when building for custom targets or using
//      non-cargo build systems that don't run the build script.
//
// With the exceptions mentioned above, the rustc-cfg emitted by the build
// script are *not* public API.

#![warn(rust_2018_idioms)]

use std::env;

include!("no_atomic.rs");

fn main() {
    let target = match env::var("TARGET") {
        Ok(target) => target,
        Err(e) => {
            println!(
                "cargo:warning={}: unable to get TARGET environment variable: {}",
                env!("CARGO_PKG_NAME"),
                e
            );
            return;
        }
    };

    // Note that this is `no_*`, not `has_*`. This allows treating
    // "max-atomic-width" as 64 when the build script doesn't run. This is
    // needed for compatibility with non-cargo build systems that don't run the
    // build script.
    if NO_ATOMIC_64.contains(&&*target) {
        println!("cargo:rustc-cfg=crossbeam_no_atomic_64");
    } else {
        // Otherwise, assuming `"max-atomic-width" == 64` or `"max-atomic-width" == 128`.
    }

    println!("cargo:rerun-if-changed=no_atomic.rs");
}
