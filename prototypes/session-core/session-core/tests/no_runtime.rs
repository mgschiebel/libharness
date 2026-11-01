//! Validation point 4, as a test: no runtime or I/O names in the library sources or
//! manifests (by inspection of text; real enforcement is ticket #18).
const SRC: &[(&str, &str)] = &[
    ("lib.rs", include_str!("../src/lib.rs")),
    ("engine.rs", include_str!("../src/engine.rs")),
    ("session.rs", include_str!("../src/session.rs")),
    ("cancel.rs", include_str!("../src/cancel.rs")),
    ("sans-io-core", include_str!("../../sans-io-core/src/lib.rs")),
];
#[test]
fn no_runtime_or_io_names() {
    for (file, text) in SRC {
        let code: String = text.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        for bad in ["tokio", "futures::", "async_std", "smol", "std::fs", "std::net", "std::io", "std::thread", "std::process", "spawn(", "sleep", "Instant::now"] {
            // the one allowed clock read is the async driver sampling `now` for the core; the pure layers never read a clock
            let allowed = bad == "Instant::now" && *file == "session.rs";
            assert!(allowed || !code.contains(bad), "{file} mentions {bad}");
        }
    }
    for (file, text) in [("session-core", include_str!("../Cargo.toml")), ("sans-io-core", include_str!("../../sans-io-core/Cargo.toml"))] {
        for bad in ["tokio", "futures", "async-std", "smol", "reqwest", "hyper", "mio"] {
            assert!(!text.contains(bad), "{file} Cargo.toml mentions {bad}");
        }
    }
}
