# PROTOTYPE (throwaway): no_std core under a std-less target gate (#22)

Question: can the libharness core (capability registry of boxed async callbacks, typed JSON values, typed results with JSON schema) be `#![no_std]` + `alloc`, `#![forbid(unsafe_code)]`, gated by `cargo check --target wasm32v1-none`, with the dependencies we need, and what does it cost? Candidate 1 of research/no-io-enforcement.md (sections 6, 14).

Verdict: yes. Everything built, including `schemars` and `tracing`. Not the session model (#19); callback shape (`BoxFut`, `Capabilities<R>` builder, `tool_typed`, typed `R`) is copied from `prototype/session-task`, minus cancel tokens/events, and `run_once` is a 20-line stand-in driver.

## Run

```
./ci.sh        # the CI-style gate (needs wasm32v1-none target, cargo-deny, python3)
./inject.sh    # injects each forbidden thing, runs ci.sh, restores; every run must FAIL
cargo test -p host-tests    # same core, native, futures executor
```

Layout: `core/` (nostd-core), `probe/` (cdylib exercising the API, wasm import check), `host-tests/` (native std test), `deny.toml`, `imports.py` (wasm import-section reader), `nobuild.py`.

## Validation point 1: core builds (wasm32v1-none, no_std, forbid(unsafe_code))

| Need | Crate (all `default-features = false`) | no_std + alloc? |
|---|---|---|
| serde derive | serde 1 (`derive`,`alloc`) | yes |
| JSON values | serde_json 1 (`alloc`) | yes |
| JSON schema (derive) | schemars 1.2.3 (`derive`) | yes, on by default feature `schemars` |
| JSON schema (hand-rolled fallback) | `HasSchema` trait + `serde_json::json!` | yes, `--no-default-features` |
| error type | thiserror 2 | yes (needs `default-features = false`) |
| futures | `core::future::Future`, futures-core 0.3 (`alloc`) | yes |
| boxed callbacks | `alloc::boxed::Box`, `alloc::sync::Arc`, `Send + Sync` | yes |
| tracing | tracing 0.1 (`attributes`) | yes (`info!`/`debug!` compile) |
| HashMap | hashbrown 0.15 (`default-hasher`) | yes |
| Mutex | spin 0.9 (`mutex`,`spin_mutex`) | yes |

Both schema variants pass `cargo check -p nostd-core --target wasm32v1-none`. The research's open question is answered: serde_json, schemars and tracing all support no_std + alloc today.

## Validation point 2: CI script (`ci.sh`)

Steps: (1) check core on wasm32v1-none; (1b) same with `--no-default-features` (hand-rolled schema); (2) `cargo deny --exclude host-tests check bans` (allowlist + `allow-build-scripts` + `executables/interpreted = deny`); (2b) `nobuild.py` (no build script in workspace members); (3) build probe cdylib, require zero wasm imports and a `run` export.

Clean tree: all PASS. Injected failures (each exits non-zero; which steps caught it):

| Injected | Result | Caught by |
|---|---|---|
| `glob` dep + use | FAIL | 1, 1b (E0463 std missing), 2 (not in allowlist), 3 |
| `extern crate std;` | FAIL | 1, 1b (E0463), 3. deny passes (no dep change) |
| `getrandom` (default-features off) | FAIL | 1, 1b (compile_error: wasm32 unsupported), 2, 3 |
| `libc` declared, unused | FAIL | 2 only. Compiles fine on wasm32v1-none (libc is no_std) |
| `unsafe extern "C" {}` block, uncalled | FAIL | 1, 1b, 3 ("usage of an `unsafe extern` block", from forbid(unsafe_code)) |
| `unsafe extern` + call | FAIL | 1, 1b, 3 |
| `build.rs` in core (workspace member) | FAIL | 2b only. cargo-deny passes it |
| dependency with `build.rs` (allowlisted by name) | FAIL | 2 (`build-script-not-allowed`) |

Extra: in the probe (unsafe permitted there), an `extern` with `#[link(wasm_import_module = "host")]` yields `imports=1 host.host_write` and the import check fails; without the attribute the link fails (`undefined symbol`). Probe clean: `imports=0 exports=['memory','run']`.

## Validation point 3: native host

`host-tests/tests/native.rs` runs the same `demo::caps()` (no code change, same crate) with `futures::executor::block_on` and a `futures::StreamExt` collect over the core's `VecStream`: passes. Typed result decoded to `Answer`, tool called once, injected clock returns 42, schema is an object.

## Measurements

| Metric | Value |
|---|---|
| Crates in wasm tree (core, normal+build, incl. proc-macro stack) | 28 distinct, 27 external |
| Allowlist size (deny.toml `allow`) | 29 names (27 external + nostd-core, probe) |
| Build-script allowlist | 13 crates |
| Native host-tests tree (for comparison, adds futures + serde std) | 38 distinct |
| core source | 167 lines lib.rs + 13 schema.rs (includes demo module of about 45) |
| no_std/cfg glue | 4 cfg lines in lib.rs (schemars derives), 5 in schema.rs, `#![no_std]`, `extern crate alloc`, alloc imports, hashbrown/spin: roughly 15 lines |
| Gate scripts | ci.sh 13, deny.toml 29, imports.py 30, nobuild.py 6 |
| `cargo check` core wasm, cold | 24 s (schemars), 19 s (without) |
| Probe release cdylib cold | 22 s; 104 KB wasm (opt-level s, lto, includes serde_json + schemars) |
| Full `ci.sh` cold | 38 s |
| Native `cargo test -p host-tests` cold | 15 s |

Features given up and workarounds:
- `HashMap` -> `hashbrown` (one extra crate, plus foldhash). `BTreeMap` would need none.
- `Mutex` -> `spin::Mutex` (spin-lock, fine for short sections; no blocking wait). `Arc` is in `alloc`.
- Time: no `Instant`; injected as a `clock: Fn() -> u64` capability (`now_ms`). Matches sans-IO.
- `std::error::Error` impl: thiserror 2 provides it via `core::error::Error`.
- Typed results and JSON schema survive in full; schemars derive works unchanged.
- Nothing forced `std` back in.

## Surprises

1. schemars 1.x and tracing work without std; no fork or hand-rolled schema needed (hand-rolled fallback kept anyway, 13 lines of glue).
2. `cargo-deny` does not apply `allow-build-scripts` to workspace members: `core/build.rs` passed deny. Needed `nobuild.py`. Non-member path deps are caught.
3. `external-default-features = "deny"` fails on the host-only proc-macro stack (proc-macro2, quote, syn, serde_derive); set to `allow`. The wasm check already covers real default-feature std use.
4. `libc` as a declared-but-unused dependency compiles on wasm32v1-none; only the allowlist catches it. `getrandom` is rejected by its own compile_error on this target.
5. `unsafe extern` blocks are rejected by `#![forbid(unsafe_code)]` even when never called, so the import-section check is a second line of defence mainly for non-core crates.
6. Probe cdylib itself needs `unsafe` (a `GlobalAlloc` bump allocator; the core stays unsafe-free) and a `panic_handler`. `core::task::Waker::noop()` gives a runtime-free executor for the probe.
7. `extern crate std;` is not caught by deny; only the target check catches it.
8. Workspace feature unification: building host-tests with core in one invocation turns on serde/serde_json `std` for the native build of core. The gate uses `-p nostd-core --target wasm32v1-none`, so it is unaffected, but a native-only build no longer proves no_std.

## Assumptions

- No one was available to steer; chose `schemars` on by default with a hand-rolled fallback feature rather than picking one.
- The session model is out of scope; `run_once` and `demo` only exercise the registry.
- Single-threaded wasm: `Arc`/`Send + Sync` bounds kept to match session-task even though wasm32v1-none has no threads.
- Allowlist is by crate name, not version; `multiple-versions = "allow"` (syn 2 and 3 coexist).
- Dev profile `panic = "abort"` is set so the probe cdylib builds; tests are unaffected.
- cargo-deny 0.20.2, rustc/cargo 1.97, wasm32v1-none from rustup.

## Recommendation for #24 (non-binding)

Adopt candidate 1. The core can live in no_std + alloc with the dependencies we need at modest cost (about 27 external crates, injected clock, hashbrown/spin). Keep the gate as: check on wasm32v1-none, cargo-deny bans with allowlist and build-script rules, plus a workspace build-script check, plus the probe import check (cheap, 30 lines Python or `wasm-tools`). Keep `forbid(unsafe_code)`. Host adapters live in a separate std crate. Open items for later: gate `tracing` features, decide HashMap vs BTreeMap, and whether to pin allowlist versions.
