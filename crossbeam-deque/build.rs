// The rustc-cfg emitted by the build script are *not* public API.

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-check-cfg=cfg(crossbeam_sanitize_thread,crossbeam_sanitize_any)");

    // `cfg(sanitize = "..")` is not stabilized.
    let sanitize = env::var("CARGO_CFG_SANITIZE").unwrap_or_default();
    if !sanitize.is_empty() {
        println!("cargo:rustc-cfg=crossbeam_sanitize_any");
        if sanitize.contains("thread") {
            println!("cargo:rustc-cfg=crossbeam_sanitize_thread");
        }
    }
}
