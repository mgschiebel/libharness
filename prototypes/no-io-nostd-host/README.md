# PROTOTYPE (throwaway): no_std core embedded in a std host app (#27)

Question: does the `no_std` + `alloc` core from #22 hold up when a normal `std` host with real I/O callbacks embeds it, while only the library crate is gated?

Verdict: yes. The unchanged core ran real file and TCP I/O under tokio multi-thread and futures-executor, the wasm gate still passes in the same workspace, 64 parallel sessions were correct with `spin` locks, host `tracing` saw the core's events, and the dependency tree is coherent. The sharp edge is workspace-level, not core-level: unification and a plain `cargo build` (see Surprises).

Started from `prototype/no-io-nostd` (core, probe, host-tests, gate scripts copied). Callback shape and the scripted-model scenario come from there (which copied session-task); I did not re-read session-task further and did not touch the session model. `run_once` is still the 20-line stand-in.

## Run

```
./ci.sh                                     # the authoritative gate (unchanged logic, host excluded from deny)
cargo test -p host --release                # points 1, 4, 5
cargo run -p host --release                 # point 3 with spin::Mutex
cargo run -p host --release --features std-mutex   # point 3 with std::sync::Mutex
```
Layout: `host/` (std lib glue + bench main + `tests/host.rs`); `core/`, `probe/`, `host-tests/`, `ci.sh`, `deny.toml` as in #22.

## Validation points

| # | Point | Result | Evidence |
|---|---|---|---|
| 1 | std host links unchanged core, real I/O, tokio multi-thread and futures-executor | PASS | `real_io_tokio_multithread`: 64 sessions on 4 workers, each writes a temp file, reads it back, GETs a local `TcpListener` stub; 64 files, 64 http_ok, 64 stub hits, content verified. `real_io_futures_executor`: `block_on` plus a 4-thread `ThreadPool` with 64 sessions: all totals correct. Core source unchanged for this (see "core diff" below). |
| 2 | wasm gate still passes with host in workspace; feature unification | PASS with caveat | `./ci.sh` 5/5 PASS after a full native workspace build. Authoritative gate: `cargo check -p nostd-core --target wasm32v1-none` (plus 1b/2/2b/3). Unification table below. |
| 3 | 64 parallel sessions, spin locks, no deadlock/starvation/lost updates | PASS | 64 sessions x 2000 tool calls sharing ONE `Capabilities` (one lock), tool yields every call so tasks interleave; counter always exactly 128000; all runs finished (2-core box, 8 workers = oversubscribed, so locks are preempted while contended). Throughput table below. |
| 4 | host tracing sees core events; `std::error::Error` and `?` | PASS | Host `tracing_subscriber::registry()` layer received `nostd_core` INFO (`run_once start`) and DEBUG events. `CoreError` -> `HostError` via `#[from]` and `?`; `Box<dyn Error + Send + Sync>` from `CoreError` via `?`; `downcast_ref::<CoreError>()` works; `&CoreError as &dyn std::error::Error` compiles. |
| 5 | host with serde_json `std` + `preserve_order` gives one coherent tree | PASS | One `serde_json 1.0.154`, one `serde 1.0.229`, one `tracing`. Host asks `std`+`preserve_order`; the core stays at `alloc`. `preserve_order` reaches the core's `Value`s (schema key order is insertion order: `type, properties, required, title, $schema`; host test asserts `["z","a"]` not sorted). Duplicates exist but are harmless: hashbrown 0.15 (core) and 0.17 (indexmap), foldhash 0.1/0.2, syn 2/3. |

### Point 2: what unification does (resolver 2)

| Build | serde_json features on core | tracing / serde |
|---|---|---|
| `cargo check -p nostd-core --target wasm32v1-none` | `alloc` only | no std |
| `cargo check -p nostd-core` (native, core alone) | `alloc` only | no std |
| `cargo build` / `--workspace` (native, host in the selection) | `alloc, std, indexmap, preserve_order, default` | tracing `std`, tracing-core `std`, serde `std` |
| `cargo check --workspace --target wasm32v1-none` | FAILS (E0463, host needs std; memchr is the first to break) | n/a |
| `cargo check -p nostd-core --target wasm32v1-none --features std-mutex` | FAILS (E0463): the gate catches the comparison feature if someone enables it | n/a |

So a workspace-wide native build compiles the core with `std` features switched on by the host. It still compiles, but proves nothing about no_std, and the wasm workspace-wide command cannot be the gate. The gate is only meaningful with `-p nostd-core --target wasm32v1-none` (resolver 2 unifies only across the selected packages and target), as `ci.sh` already does. Ran `ci.sh` after the full native workspace build: all PASS (no stale-artifact effect).

### Point 3: contention (64 sessions x 2000 calls, one shared lock, 2-core machine)

| Executor / workers | spin::Mutex calls/s | std::sync::Mutex calls/s |
|---|---|---|
| tokio, 1 | 471k / 489k | 566k / 582k |
| tokio, 2 | 915k / 855k | 822k / 725k |
| tokio, 8 | 1.41M / 1.35M | 1.38M / 1.24M |
| futures ThreadPool, 1 | 1.64M / 1.47M | 1.68M / 1.70M |
| futures ThreadPool, 2 | 2.01M / 2.33M | 2.31M / 2.52M |
| futures ThreadPool, 8 | 2.16M / 2.83M | 2.16M / 2.31M |

(two runs per cell.) No measurable difference; run-to-run noise exceeds the gap. Reason: the critical section is one `u64` increment and the guard is a temporary in a single statement, never held across an `.await`, so the lock is essentially uncontended work. Caveat: this only shows the current usage is safe. A future core that holds a spin guard across an await or does non-trivial work under it would spin a worker on a preempted holder; that is the thing to ban (or use `try_lock`/atomics) rather than the lock type.

## Measurements

| Metric | Value |
|---|---|
| Host glue, `host/src/lib.rs` | 100 lines (fs tools 2, http client+stub ~25, caps/model scripts ~35, error types/yield helper ~25) |
| Host glue other | 38 lines bench `main.rs`, 82 lines `tests/host.rs` |
| Core diff vs #22 | 23 lines changed, all for the `std-mutex` comparison cfg (a `Lock` alias and `bump`/`bump_read`); zero needed for hosting. Off by default; point 1 uses the default build |
| Host normal dep tree | 66 distinct crates (core wasm tree is 28) |
| Gate | unchanged: ci.sh 5/5 PASS |

## Awkward for a std caller

- schemars: typed tools need `I: HasSchema`, so the host must depend on the same schemars major (1.x) and derive `JsonSchema` itself. Not hidden by the core; version skew would show up as a trait-bound error. Re-exporting `schemars` (or the derive) from the core would fix it.
- `run_once` discards tool results and tool errors (`let _ = ...`). My first stub bug (connection reset) was invisible until I added `eprintln!` in the callback. A real session model must surface tool errors.
- Blocking std I/O inside the callback future runs on the executor thread. Fine for the prototype (tiny); a real host would use `tokio::fs`/`spawn_blocking`, and then the callback is tokio-specific in the host, not in the core.
- Injected clock: easy (`move || t0.elapsed().as_millis() as u64`), but milliseconds as `u64` loses sub-ms and monotonic-vs-wall distinction; fine for now.
- `hashbrown` and `spin` do not leak into the public API (`tool_specs()` returns `Vec`, `calls_made()` a `u64`), so a std caller never sees them. `HashMap`-vs-`hashbrown` was a non-issue for the host.
- `serde_json` `preserve_order` changes key order of values the core produces (schemas), so output differs between host and a bare core; tests that compare JSON text would depend on host features.
- The host cannot name `BoxFut`-returning closures without `async move` clones of captured `Arc`s; ordinary boilerplate (every callback in `io_caps` clones its captures twice).

## Surprises

1. A plain `cargo build` at the workspace root failed natively: the `probe` cdylib (panic_handler + `GlobalAlloc`) conflicts with std lang items (E0152). Not a host issue, a workspace one; fixed with `default-members = ["core","host-tests","host"]` in the workspace Cargo.toml. Anything running bare `cargo build/test` in CI needs this or `--exclude probe`.
2. Unification turns on `std` in the core's deps for every native build with the host selected (table above). The gate must keep `-p nostd-core` plus the target; a "wasm build of the whole workspace" is not usable.
3. cargo-deny needs `--exclude host` as well as `host-tests` (updated in `ci.sh`), otherwise the host's tokio tree trips the allowlist.
4. A std Mutex variant requires `extern crate std` in the core under a feature; that is an additive feature a host could turn on by accident. The gate fails it (E0463), so it is caught, but a rule "no feature of the core may enable std" is worth stating.
5. `spin` showed no pathologies even at 8 workers on 2 cores.

## Assumptions

- Stub server is a thread-per-connection `std::net::TcpListener` on 127.0.0.1:0, HTTP/1.1 `Connection: close`; no external network.
- tokio multi-thread = 4 workers for the I/O test; futures-executor means `block_on` plus `ThreadPool`.
- Sessions share one `Capabilities` only in the contention bench (that is the only shared lock); the I/O test uses one per session.
- Only throughput of a counter lock was compared; no real provider latency.
- Machine has 2 cores, so the 8-worker rows are oversubscription, not scaling.
- Tool errors are swallowed by the stand-in `run_once`, so I/O success is verified through host-side atomics and files.

## Recommendation for #24 (non-binding)

The std-host arrangement works; keep the library no_std and gate only it. Keep the authoritative gate as `cargo check -p nostd-core --target wasm32v1-none` (+ deny, build-script check, probe imports), run per package, never workspace-wide. Add `default-members` (or exclude probe) so bare cargo commands work, and document that native builds with a host selected unify `std` onto the core. Keep `spin` for short sections but add a rule not to hold guards across `.await`; no need for a std-mutex cfg. Consider re-exporting the schemars derive from the core, and make the session model surface tool errors.
