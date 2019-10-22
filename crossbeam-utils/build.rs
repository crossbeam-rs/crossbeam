extern crate autocfg;

fn main() {
    let cfg = autocfg::new();
    if cfg.probe_rustc_version(1, 31) {
        println!("cargo:rustc-cfg=has_min_const_fn");
    }
}
