//! Source scan: the core names no runtime, no std, and does no I/O.
#[test]
fn core_sources_are_std_less_and_runtime_free() {
    for f in ["lib", "step", "cancel", "aio", "registry"] {
        let src = std::fs::read_to_string(format!("{}/src/{f}.rs", env!("CARGO_MANIFEST_DIR"))).unwrap();
        for bad in ["tokio", "futures", "smol", "async_std", "std::", "thread", "spawn", "sleep", "Instant", "SystemTime", "unsafe {"] {
            let hits: Vec<&str> = src.lines().filter(|l| !l.trim_start().starts_with("//")).filter(|l| l.contains(bad)).collect();
            assert!(hits.is_empty(), "{f}.rs mentions {bad}: {hits:?}");
        }
    }
    let toml = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    assert!(!toml.contains("tokio") && !toml.contains("futures"));
}
