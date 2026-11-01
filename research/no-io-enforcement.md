# Technical enforcement of no-I/O in a Rust crate

Ticket: mgschiebel/libharness#18. Researched 2026-11-01 from primary sources only (official docs, specs, source repos, published crate manifests). Links to `main`/`master` were read on that date and may drift. Toolchain used for the experiments: rustc/cargo 1.97.0, clippy 0.1.97, cargo-deny 0.20.2.

Convention: **[Source]** statements are backed by the linked source. **[Tested]** statements are results I produced locally on 2026-11-01 (setup in section 11). **[Inference]** statements are my own reasoning. **[Unknown]** marks what I did not verify.

## 0. Summary

- No single mechanism proves "no I/O". Rust has no effect system and `std` is ambient authority, so every mechanism below catches a *class* of escape and misses others. The strongest result is a layered stack, each layer closing the previous layer's hole.
- The best cheap compile-time barrier is a **target with no `std`** (`wasm32v1-none`): any crate in the tree that needs `std` fails to compile [Tested]. Its hole is `no_std` crates that do I/O through FFI (`getrandom`, `libc`), which a `forbid(unsafe_code)` on our own crate plus a `cargo-deny` allowlist covers.
- The best ground truth is the **built artifact's import section**: a `wasm32-unknown-unknown`/`wasm32v1-none` build with zero imports cannot reach the host [Source + Tested]. Its holes are dead-code elimination (unexercised code hides its imports) and that it proves the wasm build, not the native build.
- `clippy::disallowed_*` is a per-item **denylist** that only lints our own workspace crates, never dependencies [Tested]. Useful as defense in depth when we keep `std`, redundant under `no_std`.
- Adopting rig-core, genai or swiftide would **not** pass a dependency-allowlist no-I/O check (section 9).

## 1. What "no I/O" has to cover

Escape routes, so each mechanism can be scored against them:

| # | Route | Example |
|---|---|---|
| R1 | Own code calls `std` | `std::fs`, `std::net`, `std::process`, `std::env`, `std::time::{Instant,SystemTime}::now`, `std::thread::spawn`, `println!` |
| R2 | Own code calls FFI | `unsafe extern "C" { fn write(..); }` |
| R3 | A dependency does R1 | `glob` uses `std::fs`; `web-time` reads the clock |
| R4 | A dependency does R2 (even if `#![no_std]`) | `getrandom`, `libc` |
| R5 | Build-time code | `build.rs`, proc macros run arbitrary code at compile time (not runtime I/O, but a supply-chain escape) |
| R6 | A runtime dependency brings an executor or thread | `tokio`, `futures-timer`'s helper thread; breaks runtime neutrality too |
| R7 | Ambient nondeterminism that is I/O-like | clock, entropy, env vars |

## 2. cargo-deny (dependency allow/deny lists, graph)

**What it does [Source].**
- `[bans] allow = [...]`: <q>If the `allow` list has one or more entries, then any crate not in that list will be denied</q>. `deny = [...]` bans named crates, with `wrappers` (allow only specific direct dependents), `reason` and `use-instead`. [bans/cfg.md](https://github.com/EmbarkStudios/cargo-deny/blob/main/docs/src/checks/bans/cfg.md)
- `allow-workspace = true` auto-allows workspace members while external dependencies still need explicit allowlisting, so our own crates do not need listing. [same](https://github.com/EmbarkStudios/cargo-deny/blob/main/docs/src/checks/bans/cfg.md)
- Feature control: `external-default-features = "deny"`, per-crate `[[bans.features]]` with `allow`/`deny`/`exact`. [same](https://github.com/EmbarkStudios/cargo-deny/blob/main/docs/src/checks/bans/cfg.md)
- Build-time control: `[bans.build] allow-build-scripts = [...]` (an empty list means no crate may have a build script), `executables = "deny"` (reads file headers for ELF/PE/Mach-O), `include-dependencies`. [same](https://github.com/EmbarkStudios/cargo-deny/blob/main/docs/src/checks/bans/cfg.md)
- Graph construction: `[graph] targets`, `features`, `all-features`, `no-default-features`, `exclude-dev`. By default <q>cargo-deny will consider every single crate that is resolved by cargo, including target specific dependencies</q>. [checks/cfg.md](https://github.com/EmbarkStudios/cargo-deny/blob/main/docs/src/checks/cfg.md)

**Cannot enforce.**
- It sees crate *names and features*, not what a crate's code does. The docs list what the build check will not detect, e.g. <q>The crate just straight up does bad things</q>, and say it is mainly useful for native executables and scripts. [bans/cfg.md, `build`](https://github.com/EmbarkStudios/cargo-deny/blob/main/docs/src/checks/bans/cfg.md)
- **[Inference]** So an allowlisted crate may do any I/O, and our own crate's `std::fs` call is invisible to it (R1, R2). It addresses R3, R4, R5, R6 only as far as the allowlist is curated correctly.

**Ease.** Low to adopt (one `deny.toml`, one CI step, `cargo install cargo-deny`). **Ongoing cost [Tested]:** with a 15-name allowlist of obviously pure crates (serde family, thiserror, syn/quote, etc.), the *no-default-features* trees of rig-core, genai and swiftide yield 92, 134 and 161 crates "not explicitly allowed". Every new transitive dependency or version of the dependency set is a review event. That is the feature, and the cost.

**Failure modes.**
- Allowlisting by name trusts that crate for all versions unless a version is pinned in the spec.
- Feature unification: another crate in the workspace can enable a feature that pulls in an I/O dependency. Pin the check with `[graph]` features or `external-default-features = "deny"`.
- Default graph includes all targets, so Windows/macOS-only crates (`windows-sys`) appear unless `targets` is set. Setting `targets` to only wasm32 would hide the native-only I/O crates, which is wrong for a library that also builds natively. **[Inference]** Check both a native and a wasm target.

## 3. cargo-vet (audited supply chain)

**What it does [Source].**
- Records human audits against **criteria** and blocks unaudited dependencies in CI. Setup is cheap: <q>does not require auditing the existing dependencies. These are automatically added to the exemptions list</q>. [how-it-works.md](https://github.com/mozilla/cargo-vet/blob/main/book/src/how-it-works.md)
- Built-in criteria `safe-to-run` (no <q>reading or writing data from sensitive or unrelated parts of the filesystem</q>, no connecting to untrusted endpoints) and `safe-to-deploy` (auditors must <q>reason about the behavior of all unsafe blocks and usage of powerful imports</q>, with <q>ambient capabilities (e.g. filesystem access)</q> hardened). [safe-to-run](https://github.com/mozilla/cargo-vet/blob/main/src/criteria/safe-to-run.txt), [safe-to-deploy](https://github.com/mozilla/cargo-vet/blob/main/src/criteria/safe-to-deploy.txt)
- Custom criteria are free-text labels in `audits.toml`, and policy can require different criteria per crate and per dependency edge. Google ships a `ub-risk-N` set as an example. [audit-criteria.md](https://github.com/mozilla/cargo-vet/blob/main/book/src/audit-criteria.md)

**Cannot enforce.** Nothing is machine-checked: a criterion such as a custom `performs-no-io` is only a promise by a human auditor. **[Inference]** It is the right tool for the *residual* risk (a reviewed crate that is allowlisted), not for detecting I/O.

**Ease.** Medium. `cargo vet init` is five minutes, but a real `performs-no-io` criterion means someone audits every dependency, and for a no-I/O library that is a long-tail cost. Imports from other organisations' audits only cover `safe-to-*`, not our custom criterion. **[Unknown]** how many audits for the candidate tree exist in public registries.

**Failure modes.** Exemptions made at `init` become permanent blind spots unless burned down; auditors are fallible; criteria wording must be identical across repos if aggregated.

## 4. clippy `disallowed_methods` / `disallowed_types` / `disallowed_macros`

**What it does [Source].**
- Configured in `clippy.toml` as fully qualified paths with optional `reason`, `replacement`, `allow-invalid`. [lint_configuration.md](https://github.com/rust-lang/rust-clippy/blob/master/book/src/lint_configuration.md)
- Doc examples already include `std::time::Instant::now`, `std::net::TcpListener`. The lints are warn-by-default but <q>will only trigger if</q> items are listed. [disallowed_items.rs](https://github.com/rust-lang/rust-clippy/blob/master/clippy_lints/src/disallowed_items.rs)
- Make them hard errors with `#![deny(clippy::disallowed_methods, clippy::disallowed_types, clippy::disallowed_macros)]` or `[lints.clippy]` in `Cargo.toml` (lint levels per [rustc book](https://github.com/rust-lang/rust/blob/master/src/doc/rustc/src/lints/levels.md)).

**[Tested] behaviour (clippy 0.1.97, crate `exp1`):**
- Caught: `std::fs::read_to_string` (including when stored in a local `let rts = std::fs::read_to_string;`), `std::fs::File::open` (method and the type), `std::env::var`, `Instant::now`, `std::net::TcpStream::connect`, `std::process::Command::new`, `std::thread::spawn`, `println!`.
- **Missed anything not listed**: `std::fs::OpenOptions::open` and `std::fs::read_dir` passed silently. It is an enumerated denylist, so completeness is our job.
- **Module-level entries are rejected** in this version: `{ path = "std::fs" }` gives `expected a function, found a module`. Every function and type must be named.
- **Dependencies are never linted**: `glob` (uses `std::fs::read_dir`) and `web-time` (uses `SystemTime::now`) pulled in by rig-core produced no diagnostics with both functions on the list.

**Cannot enforce.** R2 (FFI) is not covered, R3/R4 not at all, macro-generated calls only if the macro itself is listed or expands to a listed path, and it is bypassed by `#[allow]` unless the level is `forbid`. **[Inference]** `clippy.toml` is also a convention file: a contributor can edit it, as they can edit `deny.toml`. Both need CODEOWNERS or equivalent.

**Ease.** Very low (one file). **Failure mode:** false confidence from an incomplete list. Complement: the `std_instead_of_core`/`std_instead_of_alloc`/`alloc_instead_of_core` restriction lints keep a crate `no_std`-compatible. [std_instead_of_core.rs](https://github.com/rust-lang/rust-clippy/blob/master/clippy_lints/src/std_instead_of_core.rs)

## 5. `#![forbid(unsafe_code)]`

**What it does [Source].**
- `forbid(C)` is <q>the same as `deny(C)`, but also forbids changing the lint level afterwards</q>. [Reference: diagnostics](https://github.com/rust-lang/reference/blob/master/src/attributes/diagnostics.md)
- The `unsafe_code` lint covers `unsafe` blocks plus `no_mangle`, `export_name`, `link_section` attributes. [rustc lint list](https://doc.rust-lang.org/rustc/lints/listing/allowed-by-default.html#unsafe-code)
- Calling a function declared in an extern block requires an `unsafe` context (functions are implicitly `unsafe` unless marked `safe` in an `unsafe extern` block). [Reference: external blocks](https://github.com/rust-lang/reference/blob/master/src/items/external-blocks.md)

**[Tested]:** with `#![forbid(unsafe_code)]`, a crate declaring `unsafe extern "C" { fn host_write(..); }` and calling it fails with three errors (`unsafe extern` block, `unsafe` block, `no_mangle`). This closes R2 for our own crate.

**Cannot enforce.**
- It applies to one crate. Cargo passes `--cap-lints allow` to non-workspace dependencies, so it says nothing about R3/R4. [rustc book: capping lints](https://github.com/rust-lang/rust/blob/master/src/doc/rustc/src/lints/levels.md), which also notes `--cap-lints` can downgrade `forbid`.
- It does not stop `std` I/O, which is safe Rust (R1). The `cap-std` README makes the same point about untrusted code: it <q>could use `unsafe` or the unsandboxed APIs in `std::fs`</q>. [cap-std README](https://github.com/bytecodealliance/cap-std/blob/main/README.md)

**Ease.** Trivial. Zero cost unless we need `unsafe` (e.g. a `Pin` projection; use `pin-project-lite` in an allowlisted dependency instead). It does not stop us calling safe wrappers of an allowlisted FFI crate.

## 6. `no_std` + `alloc` as an I/O barrier

**What it does [Source].**
- `#![no_std]` stops `std` linking automatically and switches the prelude to `core`. The Reference says it is for crates <q>purposefully not using the capabilities of the standard library</q>, mainly allocation and <q>file and network capabilities (e.g. `std::fs` and `std::io`)</q>. [Reference: preludes](https://github.com/rust-lang/reference/blob/master/src/names/preludes.md)
- `core` <q>links to no upstream libraries, no system libraries, and no libc</q> and has no <q>concurrency or I/O</q>. [core/src/lib.rs](https://github.com/rust-lang/rust/blob/master/library/core/src/lib.rs) `alloc` adds `Box`, `Rc`, collections. [alloc/src/lib.rs](https://github.com/rust-lang/rust/blob/master/library/alloc/src/lib.rs) `core::future::Future` and `core::task` are in `core`, so async callbacks need no runtime crate. **[Inference]** There is no clock in `core`: `Instant` is `std`, so time becomes an explicit input, matching the sans-IO finding in `research/sans-io-rust.md`.

**Limits [Source].** The Reference states: <q>Using `no_std` does not prevent the standard library from being linked. It is still valid to write `extern crate std` in the crate or in one of its dependencies</q>. [preludes](https://github.com/rust-lang/reference/blob/master/src/names/preludes.md)
- **[Tested]** A `#![no_std]` + `extern crate std;` crate calling `std::fs::read_to_string` builds.
- **[Tested]** A `#![no_std]` crate with `unsafe extern "C" { fn write(..) }` builds, and `nm -u` on the rlib shows `write` as an undefined symbol.
- `getrandom` (`#![no_std]` at [src/lib.rs:5](https://docs.rs/crate/getrandom/0.4.3/source/src/lib.rs)) obtains OS entropy through libc and syscalls; `libc` is `#![no_std]` outside `rustc-dep-of-std` ([src/lib.rs:219](https://docs.rs/crate/libc/0.2.191/source/src/lib.rs)). Both are in rig-core's dependency tree.

**Making `no_std` a real barrier: build for a target that has no `std`.**
- `wasm32v1-none` <q>Imports nothing from its host environment</q>, <q>does not support `std`, only `core` and `alloc`</q>, and compiles without `std` rather than with stubs. [platform-support/wasm32v1-none.md](https://github.com/rust-lang/rust/blob/master/src/doc/rustc/src/platform-support/wasm32v1-none.md) Tier 2, stable. `x86_64-unknown-none` is similar for native. [x86_64-unknown-none.md](https://github.com/rust-lang/rust/blob/master/src/doc/rustc/src/platform-support/x86_64-unknown-none.md)
- **[Tested]** `cargo check --target wasm32v1-none` on a crate depending on rig-core, swiftide or genai fails at the first dependency using `std` (`E0463: can't find crate for std`; `memchr`, `base64`, `bytes`, `glob`, `futures-timer`, ...). A `#![no_std]` + `alloc` crate with a boxed-future callback type compiles for it. So `cargo check --target wasm32v1-none` is a CI gate that no `std`-using crate is in the tree, including transitive ones (R1, R3, R6).
- It does not catch R4 (FFI-based `no_std` crates; they can still compile there) or R5.

**Ease.** Medium to high. Costs: no `std::collections::HashMap`, no `std::sync::Mutex`, many ecosystem crates need `default-features = false` or do not support `no_std` at all. **[Unknown]** whether `serde_json`, `schemars` (needed for typed results in #19) and the planned tracing integration support `no_std + alloc` in their current versions; this is the main prototype question. The `std_instead_of_*` clippy lints help keep the crate migratable. Host-side adapters (e.g. an OpenRouter client) would live in a separate, `std`-using crate.

## 7. `cargo tree` / `cargo metadata` CI checks

**What they can do [Source].**
- `cargo tree` has `-e normal` / `no-dev`, `--target`, `-i <pkg>` (who depends on this), `--prune`, `--format "{p} {f}"` to print features. [cargo-tree](https://doc.rust-lang.org/cargo/commands/cargo-tree.html)
- `cargo metadata` emits JSON: per package its `targets[].kind` (e.g. `custom-build`, `proc-macro`), `links`, and the resolved dependency graph with `dep_kinds`. [cargo-metadata](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html)
- Build scripts can emit `cargo::rustc-link-lib` and `cargo::rustc-link-arg`, so they can add native libraries. [build-scripts](https://doc.rust-lang.org/cargo/reference/build-scripts.html)

**[Tested]** A 10-line script over `cargo metadata` listed, for rig-core without default features: 15 build-script crates (including `getrandom`, `libc`, `serde`, `mime_guess`), 11 proc-macro crates, and no `links` entries. Notably `libc` did **not** show up as `links`.

**Cannot enforce.** It only reports the graph; judging "does this crate do I/O" is still a human or a denylist. **[Inference]** Strictly a subset of what `cargo-deny` does (bans, features, build scripts, with maintained diagnostics), so a custom script is only worth it for checks cargo-deny lacks (e.g. "no crate has a `links` key"). **Ease:** trivial. **Failure mode:** same name-level blindness; platform filtering (`--filter-platform`) must cover every target.

## 8. Artifact inspection (symbols and imports)

**WebAssembly import section [Source].**
- A Wasm module has <q>no syscall instructions or built-in I/O facilities</q>; programs interact with the outside through imported capabilities. [WASI: Capabilities](https://github.com/WebAssembly/WASI/blob/main/docs/Capabilities.md) Each module runs sandboxed and <q>can't escape the sandbox without going through appropriate APIs</q>. [design/Security.md](https://github.com/WebAssembly/design/blob/main/Security.md)
- `wasm32-unknown-unknown` <q>does not import any functions from the host for the standard library</q>; <q>`std::fs` always return errors, and `std::thread::spawn` will panic. There is no means by which this can be overridden.</q> [wasm32-unknown-unknown.md](https://github.com/rust-lang/rust/blob/master/src/doc/rustc/src/platform-support/wasm32-unknown-unknown.md)
- Inspecting imports: `wasm-tools print`, `wasm-tools objdump`, `wasm-tools component wit`. [wasm-tools README](https://github.com/bytecodealliance/wasm-tools/blob/main/README.md) `wasm-objdump` from wabt does the same. [wabt README](https://github.com/WebAssembly/wabt/blob/main/README.md)

**[Tested]** (import section read with a small parser I wrote, since wasm-tools was not installed):
- A cdylib calling `std::fs::read_to_string` and `SystemTime::now()` on `wasm32-unknown-unknown`: builds, **0 imports**. So on this target `std` I/O is silently stubbed, not rejected at compile time; the import check passes while the code would fail at runtime.
- A crate calling an `unsafe extern "C"` function with no `#[link(wasm_import_module)]`: fails at link time with `undefined symbol`. With `#[link(wasm_import_module = "host")]`: builds with 1 import, `host.host_write`. So FFI shows up exactly in the import section.
- rig-core 0.46.0 (`default-features = false`) compiled into a cdylib that calls `rig_core::now_ms()` on `wasm32-unknown-unknown`: **8 imports** (`__wbindgen_placeholder__.__wbg_now_...`, `__wbindgen_throw`, externref helpers). rig-core's manifest enables `uuid`'s `js` feature and `futures-timer`'s `wasm-bindgen` feature for `wasm32` + `target_os = "unknown"`, so the import check fails it ([Cargo.toml](https://docs.rs/crate/rig-core/0.46.0/source/Cargo.toml)).

**Native symbol inspection [Tested].** `nm -D -u` on a `std`-using cdylib lists libc entries (`open64`, `read`, `write`, `getenv`, `malloc`, ...) because `std` links libc. Noise is high; the check is only usable on a `no_std` rlib, where it showed only alloc shims (`__rust_alloc`, `handle_error`) and, for an FFI call, `write`. **[Inference]** Native symbol checks are therefore only worth it on the no_std crate's rlib; the wasm import check is cleaner.

**Cannot enforce.** Dead-code elimination: `wasm-ld --gc-sections` drops unexported, uncalled code and its imports, so the probe must exercise the library's public API. It proves one build config, not all features. It says nothing about host-side code: build scripts and proc macros run on the host at compile time (R5).

**Ease.** Medium: a probe cdylib crate, one extra target, a few lines of CI. **Failure modes:** feature or `cfg` differences between the probe and real use; `wasm-bindgen` imports being mistaken for "just JS glue".

## 9. Would rig-core / genai / swiftide pass a dependency-allowlist check?

**Short answer: no.** All numbers from published manifests and `cargo tree`/`cargo-deny` runs on 2026-11-01 (rig-core 0.46.0, genai 0.7.2, swiftide 0.33.0).

| Crate | Pulls an executor/HTTP stack with default features? | I/O-capable crates in tree (feature-minimal) | I/O in its own source |
|---|---|---|---|
| rig-core 0.46.0 | **No.** Default features (`derive`, `rustls`) give 105 packages with no tokio, reqwest, hyper or mio; HTTP is behind `rig-http` plus optional `rig-reqwest`/`tungstenite` features. [Cargo.toml](https://docs.rs/crate/rig-core/0.46.0/source/Cargo.toml) | `glob`, `getrandom`+`libc` (via `uuid` v4), `futures-timer`, `web-time`, `fastrand`; 92 of the feature-minimal crates not in a pure-crate allowlist | **Yes**: `std::fs` in `providers/internal/auth/store.rs`, `std::env::var` in `client/env.rs`, `std::net::TcpListener` and `std::process::Command` in `providers/chatgpt/auth/native/browser.rs`, `glob` in `loaders` |
| genai 0.7.2 | **Yes, unconditionally**: `reqwest` and `tokio` (`rt-multi-thread`, `io-std`, `time`) are non-optional. [Cargo.toml](https://docs.rs/crate/genai/0.7.2/source/Cargo.toml) | `reqwest`, `hyper`, `h2`, `tokio`, `mio`, `socket2`, `getrandom`, `libc` (134 crates not allowed) | it is a client; the HTTP I/O is the product |
| swiftide 0.33.0 (default features) | **Yes**: `swiftide-core` depends on `tokio` (`rt-multi-thread`, `time`) unconditionally; its default tree already contains `fs-err` and `walkdir`, and the optional `swiftide-agents` adds `process-wrap`, `rmcp` and more `tokio`. [swiftide-core](https://docs.rs/crate/swiftide-core/0.33.0/source/Cargo.toml), [swiftide-agents](https://docs.rs/crate/swiftide-agents/0.33.0/source/Cargo.toml) | `tokio`, `mio`, `socket2`, `walkdir`, `fs-err`, `getrandom`, `libc`, `parking_lot` (161 crates not allowed) | `fs-err`/`process-wrap` use in the `swiftide-agents` tool executors (files `tool_executor.rs`, `tools/local_executor.rs`) |

Details and caveats:
- **[Tested]** `futures-timer` 3.0.4 on native spawns a global helper thread (`native/global.rs`); `fastrand` seeds from `Instant::now()` and `getrandom`; `glob` calls `std::fs`. rig-core's `wasm32-unknown-unknown` build adds `wasm-bindgen`/`js-sys`.
- **[Tested]** On `wasm32v1-none` all three fail at the first dependency (section 6). On `wasm32-unknown-unknown` rig-core compiles (`cargo check`) but has 8 imports (section 8).
- Even if every I/O-capable dependency of rig-core were allowlisted, the allowlist would stop proving anything, because the I/O is in rig-core's own source (R1). Only clippy (own code) or `forbid`-style lints could see it, and we cannot add those to someone else's crate.
- This agrees with the design finding already recorded in the #10 comment: the ergonomic `Agent` owns model calls and tool execution, which contradicts the host-owns-side-effects model. The enforcement view adds a second, independent reason not to adopt them **as a dependency of the core crate**.
- **[Inference]** Narrower options remain. Using rig-core's *type-only* surface (messages, tool schemas) from an adapter crate would sit outside the no-I/O core, so the allowlist would not apply. Taking only `rig-http`'s client trait is plausible but unverified. **[Unknown]** I did not test `rig-core` with `default-features = false` against a stricter allowlist than the one above, or any version other than 0.46.0.

## 10. Capability-passing patterns

**What the primary sources say.**
- `cap-std` replaces ambient `File::open("/anything")` with `dir.open(..)` on a held `Dir`; it needs a root `Dir` obtained from `open_ambient_dir` (explicitly <q>not sandboxed</q>) or another source. [README](https://github.com/bytecodealliance/cap-std/blob/main/README.md)
- It states its own limit: <q>`cap-std` is not a sandbox for untrusted Rust code. Among other things, untrusted Rust code could use `unsafe` or the unsandboxed APIs in `std::fs`.</q> It lets code <q>declare its intent</q>. [README](https://github.com/bytecodealliance/cap-std/blob/main/README.md)
- WASI draws the same line at the module boundary: link-time capabilities are imported functions, runtime capabilities are handles passed around. [WASI Capabilities](https://github.com/WebAssembly/WASI/blob/main/docs/Capabilities.md)

**Application to libharness [Inference].**
- libharness is the *consumer* side of the pattern: it never holds a `Dir` or socket; the host passes async callbacks (`tools`, `skills`, `model`, plus clock and entropy as callbacks too). So `cap-std` should not be a dependency (it is itself an I/O crate); the idea transfers, the crate does not.
- A capability-passing API is a **design property, not an enforcement mechanism**: nothing stops the library from also calling `std::fs`. It reduces how often that is tempting (time and randomness become parameters, e.g. a `now` input as in the sans-IO prototype) and it makes the other mechanisms (no_std, wasm) practical. Pair it with sections 5, 6 and 2.
- `async fn` in traits is not dyn-compatible, so a registry of heterogeneous callbacks needs boxed futures (`Pin<Box<dyn Future>>`) or a similar shape. [Reference: dyn compatibility](https://github.com/rust-lang/reference/blob/master/src/items/traits.md) That is a #19 question, noted here only because boxing is allowed under `alloc`.

## 11. WASM as a hard boundary (and WASI / component model alternative)

**Two different things share the name:**
1. **Wasm as a build target for a check** (sections 6 and 8): compile libharness to `wasm32v1-none`/`wasm32-unknown-unknown` in CI and assert the import section is empty. The library itself is still shipped as a native Rust crate.
2. **Wasm as the runtime sandbox**: ship libharness as a Wasm module/component that the host loads (via wasmtime etc.). The sandbox is then enforced by the Wasm runtime, not by the Rust toolchain.

**Sourced facts.**
- Wasm modules are sandboxed and escape only through APIs the embedder provides. [Security.md](https://github.com/WebAssembly/design/blob/main/Security.md)
- `wasm32v1-none` and `wasm32-unknown-unknown` import nothing for `std` (section 8). The WASI targets (`wasm32-wasip1`/`p2`/`p3`) are the ones that more fully support `std`; **[Inference]** they do so through WASI imports, so their import section is non-empty by design. [wasm32-unknown-unknown.md](https://github.com/rust-lang/rust/blob/master/src/doc/rustc/src/platform-support/wasm32-unknown-unknown.md)
- The component model aims at <q>portable, virtualizable, statically-analyzable, capability-safe, language-agnostic interfaces</q>, with fine-grained sandboxing per component. [Goals.md](https://github.com/WebAssembly/component-model/blob/main/design/high-level/Goals.md)
- WASI 0.3 builds on 0.2 and replaces explicit streams and polling with component-model-native `async`, `future` and `stream`. [WASI README](https://github.com/WebAssembly/WASI/blob/main/README.md)

**Fit to libharness [Inference].**
- A component whose WIT world imports only `model`, `tool.call`, `clock` and `rand`-style interfaces, with no `wasi:filesystem`, `wasi:sockets` or `wasi:cli`, *cannot* name those operations, and a runtime refuses to instantiate otherwise. This is the real "hard boundary" and also matches the host-registered-callbacks idea one to one.
- Cost: callbacks become serialized WIT calls (no Rust closures or generic types across the boundary), the host needs a Wasm runtime dependency, and the typed return value from #19 must be expressed in WIT or JSON. WASI 0.3 async maturity is the unknown. The user already framed this as "possible later", and the sections 5, 6 and 8 stack is designed so that it is also the precondition for that later step (a crate that builds for `wasm32v1-none` with zero imports is already a valid guest).
- **[Unknown]** I did not verify wasmtime's async host-function story or component tooling maturity for guest libraries; those would be research for the later step.

## 12. Experiment record

All local, in throwaway crates outside the repo. rustc 1.97.0 (`wasm32v1-none` and `wasm32-unknown-unknown` targets added with `rustup target add`), clippy 0.1.97, cargo-deny 0.20.2 (`cargo install --locked`).
- `exp1`: clippy denylist (12 warnings; the missed and module-path cases above).
- `exp2`: wasm import section for `std::fs`, FFI with and without `wasm_import_module`, `forbid(unsafe_code)`, native `nm -D -u`, `wasm32v1-none` + `std` (E0463).
- `exp3`: `#![no_std]` + `alloc` boxed-future crate; `no_std` + `extern crate std`; `no_std` + FFI; `nm -u` on the rlib.
- `rig`, `rig2`, `genai`, `swift`: probe crates depending on the candidates (`=0.46.0`, `=0.7.2`, `=0.33.0`; rig/genai/swiftide with `default-features = false`, plus rig-core with defaults) for `cargo tree`, `cargo metadata`, `cargo deny check bans` with an allow list of 15 pure crates and `allow-workspace = true`, `cargo check --target wasm32v1-none`, and the wasm import dump. rig-core needs rustc 1.97.1 (as noted in the #10 comment), so these used `--ignore-rust-version` on 1.97.0.

## 13. Comparison

Columns: which escape route the mechanism closes (R-numbers from section 1). "Hard" means a violation fails the build with no per-case judgement.

| Mechanism | Closes | Misses | Strength | Adoption cost | Main failure mode |
|---|---|---|---|---|---|
| cargo-deny allowlist (+ features, build scripts) | R3, R4, R5, R6 (name level) | R1, R2 in own and allowlisted crates | Hard on crate names | Low setup, ongoing review per new dependency | Allowlisted crate does I/O; feature unification |
| cargo-vet `performs-no-io` criterion | R3-R5 (human judgement) | everything unaudited; nothing machine-checked | Soft | Medium to high (audits) | Exemptions never burned down |
| clippy `disallowed_*` | R1 in own crate (listed items) | R2-R6; unlisted items; deps never linted | Hard but leaky (denylist) | Very low | Incomplete list; no module-level paths |
| `forbid(unsafe_code)` | R2 in own crate | R1, R3, R4 | Hard | Trivial | Only one crate |
| `no_std` + alloc (attribute only) | R1 (own crate) | `extern crate std`, FFI, deps | Convention-grade | Medium | Escape hatch is one line |
| Build for a no-std target (`wasm32v1-none`) | R1, R3, R6 across whole tree | R4 (no_std FFI crates), R5 | Hard | Medium to high (`no_std` ecosystem) | Deps lacking `no_std` support |
| `cargo tree`/`metadata` script | reports graph | judging what crates do | Soft (subset of cargo-deny) | Trivial | Name-blind |
| Wasm import-section check | R1-R4, R7 in the built probe | DCE hides unexercised code; one build config; R5 | Hard (ground truth for that build) | Medium | Probe not exercising the API; wasm-bindgen cfg differences |
| Capability-passing API (cap-std style, trait-injected) | reduces temptation for R1, R7 | enforcement, any bypass | Design only | Design cost | Not enforced by itself |
| Wasm/component runtime sandbox | all I/O at runtime | host-side build code (R5) | Hardest | High (architecture change) | Serialization boundary, async maturity |

## 14. Leading candidates

**[Inference]** (the sources establish each tool's behaviour; the ranking is mine).

1. **No-std-target gate + `forbid(unsafe_code)` + cargo-deny allowlist.** Build the core crate `#![no_std]` + `alloc`, `#![forbid(unsafe_code)]`, with CI running `cargo check --target wasm32v1-none` (std-free tree, hard) and `cargo deny check bans` with `allow`, `allow-build-scripts` and `external-default-features = "deny"` (covers FFI `no_std` crates and build scripts). Strongest compile-time story and the natural runtime-neutral shape (`core::future`). Uncertainty: can serde/serde_json/schemars (typed results) and the other must-have dependencies live in `no_std + alloc`?
2. **`std` + clippy denylist + `forbid(unsafe_code)` + cargo-deny allowlist.** Fallback if candidate 1 is too costly. Own-code R1 is covered only for listed items, and the allowlist must exclude every I/O crate by review. Weaker but keeps `std::collections` and `Mutex`.
3. **Wasm import-section probe** on top of either (cheap, ground truth for the guest build). Largely validated already in section 8; the remaining question is the coverage cost of keeping the probe honest, which does not change a decision.

Not leading: cargo-vet (high cost, human-only; revisit only if the allowlist grows), a custom `cargo metadata` script (subset of cargo-deny), cap-std as a dependency (it is an I/O crate). The Wasm component runtime is the later "hard boundary", out of this round.

## 15. Sources not examined / gaps

- [Unknown] No primary source on no-I/O enforcement specifically for LLM agent libraries.
- [Unknown] `no_std` support of `serde_json`, `schemars`, `tracing`, `thiserror` at current versions, not checked; this is prototype A below.
- [Unknown] Did not examine `cargo-geiger`, `cargo-nono` or similar third-party tools, and did not test `cargo-vet` itself. Not primary sources for the question, and cargo-vet's criteria are documented as human-judged.
- [Unknown] Clippy `disallowed_*` behaviour on future versions: the source on `master` is named `disallowed_items.rs` and may gain module-level paths.
- Fetching `docs.rs` rendered pages was not possible from the shell; I read published crate manifests and source tarballs from `static.crates.io` and docs from the upstream repositories instead. The `docs.rs/crate/.../source` links above point at the same files.
