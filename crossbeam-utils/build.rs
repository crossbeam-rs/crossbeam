// The rustc-cfg emitted by the build script are *not* public API.

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!(
        "cargo:rustc-check-cfg=cfg(crossbeam_sanitize_thread,crossbeam_atomic_cell_force_fallback)"
    );

    let target = &*env::var("TARGET").expect("TARGET not set");

    // `cfg(sanitize = "..")` is not stabilized.
    // x86_64-unknown-linux-gnuasan is available on stable since Rust 1.95: https://github.com/rust-lang/rust/pull/149644
    // x86_64-unknown-linux-gnu{t,m}san is available on stable since Rust 1.96: https://github.com/rust-lang/rust/pull/152757
    let sanitize = env::var("CARGO_CFG_SANITIZE").unwrap_or_default();
    let sanitize_thread = sanitize.contains("thread") || target == "x86_64-unknown-linux-gnutsan";
    if sanitize_thread {
        println!("cargo:rustc-cfg=crossbeam_sanitize_thread");
    }
    if sanitize_thread
        || !sanitize.is_empty()
        || target == "x86_64-unknown-linux-gnuasan"
        || target == "x86_64-unknown-linux-gnumsan"
    {
        println!("cargo:rustc-cfg=crossbeam_atomic_cell_force_fallback");
    }
}
