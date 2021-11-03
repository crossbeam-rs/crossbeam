#![warn(rust_2018_idioms)]

use std::env;

include!("no_atomic.rs");

// The rustc-cfg listed below are considered public API, but it is *unstable*
// and outside of the normal semver guarantees:
//
// - `crossbeam_no_atomic_cas`
//      Assume the target does *not* support atomic CAS operations.
//      This is usually detected automatically by the build script, but you may
//      need to enable it manually when building for custom targets or using
//      non-cargo build systems that don't run the build script.
//
// - `crossbeam_no_atomic`
//      Assume the target does *not* support any atomic operations.
//      This is usually detected automatically by the build script, but you may
//      need to enable it manually when building for custom targets or using
//      non-cargo build systems that don't run the build script.
//
// - `crossbeam_no_atomic_64`
//      Assume the target does *not* support AtomicU64/AtomicI64.
//      This is usually detected automatically by the build script, but you may
//      need to enable it manually when building for custom targets or using
//      non-cargo build systems that don't run the build script.
//
// With the exceptions mentioned above, the rustc-cfg strings below are
// *not* public API. Please let us know by opening a GitHub issue if your build
// environment requires some way to enable these cfgs other than by executing
// our build script.
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

    let target_arch = target.split('-').next().unwrap_or_default();

    // Note that this is `no_*`, not `has_*`. This allows treating
    // `cfg(target_has_atomic = "ptr")` as true when the build script doesn't
    // run. This is needed for compatibility with non-cargo build systems that
    // don't run the build script.
    if NO_ATOMIC_CAS
        .iter()
        .any(|t| t.split('-').next().unwrap_or_default() == target_arch)
    {
        println!("cargo:rustc-cfg=crossbeam_no_atomic_cas");
    }
    if NO_ATOMIC
        .iter()
        .any(|t| t.split('-').next().unwrap_or_default() == target_arch)
    {
        println!("cargo:rustc-cfg=crossbeam_no_atomic");
        println!("cargo:rustc-cfg=crossbeam_no_atomic_64");
    } else if NO_ATOMIC_64
        .iter()
        .any(|t| t.split('-').next().unwrap_or_default() == target_arch)
    {
        println!("cargo:rustc-cfg=crossbeam_no_atomic_64");
    } else {
        // Otherwise, assuming `"max-atomic-width" == 64`.
    }

    println!("cargo:rerun-if-changed=no_atomic.rs");
}
