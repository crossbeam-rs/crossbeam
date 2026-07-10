// The rustc-cfg emitted by the build script are *not* public API.

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!(
        "cargo:rustc-check-cfg=cfg(crossbeam_sanitize_thread,crossbeam_atomic_cell_force_fallback)"
    );

    // `cfg(sanitize = "..")` is not stabilized.
    if let Ok(sanitize) = env::var("CARGO_CFG_SANITIZE") {
        if sanitize.contains("thread") {
            println!("cargo:rustc-cfg=crossbeam_sanitize_thread");
        }
        println!("cargo:rustc-cfg=crossbeam_atomic_cell_force_fallback");
    }
}
