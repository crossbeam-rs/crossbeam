fn main() {
    let cfg = autocfg::new();
    cfg.emit_type_cfg("core::sync::atomic::AtomicU8", "has_atomic_u8");
    cfg.emit_type_cfg("core::sync::atomic::AtomicU16", "has_atomic_u16");
    cfg.emit_type_cfg("core::sync::atomic::AtomicU32", "has_atomic_u32");
    cfg.emit_type_cfg("core::sync::atomic::AtomicU64", "has_atomic_u64");
    cfg.emit_type_cfg("core::sync::atomic::AtomicU128", "has_atomic_u128");
}
