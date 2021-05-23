#![warn(rust_2018_idioms)]

use std::env;

use autocfg::AutoCfg;

include!("no_atomic.rs");

// The rustc-cfg strings below are *not* public API. Please let us know by
// opening a GitHub issue if your build environment requires some way to enable
// these cfgs other than by executing our build script.
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
    // `cfg(target_has_atomic = "ptr")` as true when the build script doesn't
    // run. This is needed for compatibility with non-cargo build systems that
    // don't run the build script.
    if NO_ATOMIC_CAS.contains(&&*target) {
        println!("cargo:rustc-cfg=crossbeam_no_atomic_cas");
    }
    if NO_ATOMIC.contains(&&*target) {
        println!("cargo:rustc-cfg=crossbeam_no_atomic");
    }

    let cfg = match AutoCfg::new() {
        Ok(cfg) => cfg,
        Err(e) => {
            println!(
                "cargo:warning={}: unable to determine rustc version: {}",
                env!("CARGO_PKG_NAME"),
                e
            );
            return;
        }
    };

    cfg.emit_type_cfg("core::sync::atomic::AtomicU8", "has_atomic_u8");
    cfg.emit_type_cfg("core::sync::atomic::AtomicU16", "has_atomic_u16");
    cfg.emit_type_cfg("core::sync::atomic::AtomicU32", "has_atomic_u32");
    cfg.emit_type_cfg("core::sync::atomic::AtomicU64", "has_atomic_u64");
    cfg.emit_type_cfg("core::sync::atomic::AtomicU128", "has_atomic_u128");

    println!("cargo:rerun-if-changed=no_atomic.rs");
}
