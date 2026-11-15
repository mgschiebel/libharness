# No-I/O gate: negative checks

The gate in `ci/gate.sh` (ADR 0002) is only worth trusting if it has been seen to fail. `ci/negative-checks.sh` injects one deliberate violation at a time, runs the gate, and requires the layer meant to catch it to report `FAIL`. It starts with a positive control (the clean tree must pass), restores every file it touches, and exits non-zero if any violation slips through. CI runs it as the `negative-checks` job.

Last recorded run: 2026-11-15, rustc 1.97.0, cargo-deny 0.20.2, on the stub core (ticket 39). 13 of 13 violations caught.

## Layers

| Layer | What it checks |
|---|---|
| 1 | `ci/crates.py`: every crate declares `gated = true` or `false`; a gated crate has no build script and its library root carries `#![no_std]` and `#![forbid(unsafe_code)]` |
| 2 | `cargo check -p <crate> --target wasm32v1-none`, per package, never workspace-wide |
| 3 | `cargo deny check bans licenses`: allowlist of crate names, build-script allowlist, permissive-only licences, ungated crates excluded |
| 4 | `cargo clippy` with `await_holding_invalid_type` for `spin` guards (`clippy.toml`), warnings denied |
| 5 | build `libharness-probe` for `wasm32v1-none` and require zero wasm imports and a `run` export (`ci/wasm_imports.py`) |

## Results

"Intended layer" is the layer the violation targets and which must fail. Other layers often fail as well; that is not a miss.

| Violation | Intended layer | Also failed |
|---|---|---|
| `extern crate std;` in a gated crate | 2 | 4, 5 |
| std-only dependency (`glob`) | 2 | 3, 4, 5 |
| I/O-capable dependency that still compiles `no_std` (`libc`, declared, unused) | 3 | none: layer 3 alone catches it |
| own build script in a gated crate | 1 | none: cargo-deny does not cover workspace members |
| dependency with a build script (name allowlisted, build script not) | 3 | none |
| dependency with a non-permissive licence (`option-ext`, MPL-2.0; name allowlisted) | 3 | 2, 4, 5 (it needs `std`) |
| `spin` guard held across an await | 4 | none |
| `unsafe` block in a gated crate | 2 | 4, 5 |
| `#![forbid(unsafe_code)]` removed | 1 | none |
| `#![no_std]` removed | 1 | 2, 4, 5 |
| gated crate loses its `gated` declaration | 1 | none |
| crate declares neither `gated = true` nor `false` (the probe) | 1 | 3 |
| probe with a wasm import | 5 | none |

## Known limits

- Layer 5 only sees code the probe calls: dead-code elimination can hide an import in code it never reaches (documented in ADR 0002, not closed).
- Layer 1's source rules are textual. They read the library root file only, so an attribute set in another way (for example `[lints]` in a manifest) is not recognised.
- A gated crate depending on the ungated ureq Transport crate is caught by layer 3 (its name is not on the allowlist). Ticket 47 adds that crate and its negative check.
