# PROTOTYPE (throwaway): callback boundary, registry shape and blocking callbacks

Ticket: mgschiebel/libharness#36. Branch `prototype/callback-boundary` (based on `prototype/single-call-core`), never merged, no PR.

**Questions:** (1) what may cross a callback boundary: serialisable data only, or data plus handles (cancel token, event sink, response-body stream)? (2) is the capability set a value the host passes to each call, or an object the library holds with callbacks registered under a name? (3) how are blocking callbacks registered so they cannot stall sibling calls or the host's main thread?

**Answer in one paragraph:** every boundary variant passes every scenario, and the registry and blocking axes separate cleanly. Data plus three plain callbacks (`cancelled()`, `emit(bytes)`, and a body read/close pair addressed by id) is enough to run the whole scenario set in-process through JSON bytes (B2) and across a real Wasm boundary under wasmtime (B3, typed imports and one generic `call(name, args)` import). What the plain form loses is the synchronous cancel hook: a library-owned token can release a child or a blocked tool at once; `cancelled()` can only be polled, and across Wasm the "poll once more after cancel" guarantee cannot cross at all, so an inline child no longer reports `Aborted` back to the tool that started it (0 of 7 in B2 and B3; fixed in B2, not in B3, by handing the callback the host's own token). Registry: a value passed per call (R1) and a context object that is snapshotted at run start (R2-snapshot) behave identically; a context looked up live during a run lets a registration change a run in flight, and a context held as one Wasm instance serialises runs. Blocking: async-only registration (K1) stalls siblings for the full blocking time (300 of 300 ms) and, with the run on the host's one thread, the host for 300 ms; a blocking registration form plus a host-registered runner (K2), or the opt-in `thread-runner` crate (K3), keeps both at 0 to 1 ms. Neither frees an uncooperative blocked thread on abort. The gated core (`cb-core`) is `no_std` + `alloc`, `forbid(unsafe_code)`, starts no threads, and passes `cargo check -p cb-core --target wasm32v1-none`. Nothing ran against a live API; the only network is a loopback listener on 127.0.0.1.

## Run

```
cd prototypes/callback-boundary
rustup target add wasm32v1-none                              # sandbox-local, once
cargo test                                                   # 83 tests (cb-host: 20 s blocking suite, 5 s boundary suite); builds the guest on demand
cargo test -p cb-host --test measure -- --nocapture --test-threads=1   # crossings, bytes, latencies, instance costs
cargo test -p cb-host --test blocking -- --nocapture --test-threads=1  # K axis tables
cargo test -p cb-host --test registry -- --nocapture                   # R axis
cargo check -p cb-core --target wasm32v1-none                # the ADR 0002 gate (per package)
cargo build -p cb-guest --target wasm32v1-none --release     # the Wasm guest (not in default-members: it only builds for wasm)
```

Reused unchanged from ticket 35 by path (`../single-call-core/call-core`): the pure transitions, `State`, `CancelToken`, `abortable`, `join_all`. `call-core` is not a member of this workspace, so its own gate runs in its own workspace (`cd ../single-call-core && cargo check -p call-core --target wasm32v1-none`: passes; see Surprises for why not from here).

## What was built

| Crate | Role | Gated (`no_std`+alloc) | Lines `wc -l` (code only) |
|---|---|---|---|
| `cb-core/src/link.rs` | `Link` (what a Loop needs from callbacks, no handle types in it), `Inbox`, the Loop (`drive`), the Exchange stand-in (`exchange`: Transport call, SSE-ish body, deltas to a sink) | yes | 167 (139) |
| `cb-core/src/b1.rs` | B1 + R1 + K2: `Caps1` (the capability set as a value, one `Arc`), `ModelCtx`/`ToolCtx1` with real handles, `tool` (async) and `tool_blocking` (+ `Runner`, `Shot`, `RegisterError`), `Run1` | yes | 227 (200) |
| `cb-core/src/b2.rs` | B2 and the library half of B3: `Wire` (bytes in, bytes out), `Plain` (the plain callbacks), `Link2`, `Body2` | yes | 121 (99) |
| `cb-core/src/context.rs` | R2: `Context` (registered into, held between calls), `snapshot_run`, `live_run` | yes | 53 (40) |
| `cb-core/src/abi.rs` | the job and result a Wasm guest takes and returns, `run_job` | yes | 36 (30) |
| `thread-runner` | K3: opt-in bounded thread pool, one `Runner` value | no (std), `gated = false` | 64 (58) |
| `cb-guest` | B3 guest: allocator, panic handler, imports, exports, a 20-line `block_on`; wraps `cb-core` | no (needs `unsafe`), `gated = false` | 162 (143) |
| `cb-host` | the host: natural callbacks (`Api`), `HostWire`, wasmtime embedding, `Rig` (one entry point for B1/B2/B3), loopback mock server, scenarios, tests | no | host 242, wire 182, wasm 293, rig 183, mock_net 111, scenarios 251 |

Glue per variant (lines a boundary costs beyond the shared Loop/Exchange in `link.rs`, 167):

| Variant | Library side | Host side |
|---|---|---|
| B1 | `Run1` Link impl 48 (inside `b1.rs`) | `Rig::caps1` 14 + `Api::from_handles` 4 = 18 |
| B2 | `b2.rs` 121 | `HostWire` 95 + `Api::from_plain` 3 = 98 (+ `CountingWire` 46 for the meter only) |
| B3 typed | `b2.rs` (shared with B2) + `abi.rs` 36 | guest crate 162, wasmtime embedding `wasm.rs` 293, `Rig` B3 arms about 50, `HostWire` 95 (shared with B2); 13 imports |
| B3 generic `call(name,args)` | same | guest `match op` 1 line instead of 6; embedding loop over 4 typed imports not needed; 10 imports |
| R1 | the `Caps1` builder, about 80 of `b1.rs` | none beyond registering |
| R2 | `context.rs` 53 | none beyond registering; in Wasm: `guest_register` export 6 + `Rig::held`/`run_held` 14 |
| K1 | 0 (only `tool`) | 0 |
| K2 | `tool_blocking` 23 + `Shot` (a no_std one-shot channel) 37 + `Runner`/`RegisterError` about 10 = about 70 | a runner: 3 lines (`std::thread::spawn(job)`), 3 lines (`tokio spawn_blocking`) |
| K3 | same as K2 | 1 line (`ThreadRunner::new(4).runner()`); crate 64 |

## Variants

One axis at a time against the baseline (B1, R1, K1); the other two axes keep their baseline value. "B1 + R2" means the boundary stays B1 while the registry changes.

| Axis | Variant | What it is |
|---|---|---|
| Boundary | **B1** (baseline) | in-process closures with handles: the callback gets a `CancelToken` (sync hooks), an event `Sink`, and a transport returns a `Box<dyn BodyStream>` whose `Drop` closes the connection |
| Boundary | **B2** | every argument, reply and event is JSON bytes, in-process; handles replaced by `Plain { cancelled(), emit(bytes) }` and body reads by `call("body_next", id)` plus `close(id)`; body chunks cross as raw bytes (JSON has no byte string) |
| Boundary | **B3 typed** | the Loop and Exchange compiled to `wasm32v1-none`, run under wasmtime 50.0.1; callbacks are host imports, one per operation kind (`model_start`, `tool_start`, `transport_start`, `body_next_start`) plus 9 shared (`op_poll`, `op_take`, `op_drop`, `wait`, `cancelled`, `emit`, `validate`, `inbox`, `body_close`) |
| Boundary | **B3 generic** | the same guest with one `call_start(name, args)` import for every operation kind |
| Registry | **R1** (baseline) | `Caps1`: name to callback, one `Arc` to clone, built by the host, passed to each run; the library keeps nothing |
| Registry | **R2 snapshot** | a `Context` the host registers into and the library holds; each run copies it at start |
| Registry | **R2 live** | same context, but a run looks callbacks up in it on every call |
| Registry | **R2 in Wasm** | a held guest instance with the registrations inside it (`guest_register`); runs go through it |
| Blocking | **K1** (baseline) | async-only registration, contract "must not block"; a turn's tool calls are polled together in one task |
| Blocking | **K2** | `tool` (async) and `tool_blocking`; a blocking callback goes to a host-registered `Runner`; `tool_blocking` with no runner registered returns `RegisterError::NoBlockingRunner` |
| Blocking | **K3** | K2 with the runner coming from the opt-in `thread-runner` crate (std, outside the gate), registered by the host |

## Scenario results

Ticket 35's scenarios, re-expressed over the new entry point (same assertions; the shell they used does not ship after ADR 0004), plus the four new ones. "pass" = asserted and passes. Counts are tests: `cargo test` 83 pass, 0 fail; the whole suite was run 5 times in a row (default parallel threads, a shared 2-core machine) with 0 failures.

| Scenario | B1 | B2 | B3 typed | B3 generic | R2 snapshot | R2 live |
|---|---|---|---|---|---|---|
| 1 typed result via `final_answer` plus a tool | pass | pass | pass | pass | pass | pass |
| 2 malformed answers: retry, `Malformed`, `NoAnswer` | pass | pass | pass | pass | pass | pass |
| 3 steer lands at the next boundary; follow-up only when stopping | pass | pass | pass | pass | pass | pass |
| 4 abort while a tool hangs: run is `Aborted`, hung callback released | pass (hook ran 7/7) | pass (no hook, released by drop) | pass (released by `op_drop`) | pass | pass | pass |
| 5 spawned child through an ordinary tool | pass | pass | pass | pass | pass | pass |
| 5b abort reaches the child, hung tool in the child released | pass, 0 ms | pass, 0 ms | pass, 1 ms | pass, 1 ms | pass | pass |
| 5c abort reaches an inline child AND the child reports `Aborted` to the tool that started it (35's "extra poll" case) | **pass 7/7** | **fail 0/7** (pass 20/20 with the host's token in `Api`) | **fail 0/7** (1/20 with the host's token) | **fail 0/7** | pass | pass |
| 5d same, child spawned on the host's executor (35's host-spawned case) | pass 7/7 | pass 7/7 (2 ms, polling thread) | pass 7/7 | pass 7/7 | pass | pass |
| 6 two runs in parallel, no shared state | pass | pass | pass (instance per run) | pass | pass | pass |
| 7 (new) model reply streamed with live deltas to a host sink (6 words, 20 ms apart; first delta at 20 ms, run ends 101 ms later) | pass | pass | pass | pass | pass | pass |
| 8 (new) Transport call, loopback TCP, streamed byte body, deltas live (8 words) | pass | pass | pass | pass | pass | pass |
| 8b (new) dropping the body cancels: server sees the hang-up (0 to 1 ms after cancel, 3 of 60 chunks sent) | pass | pass | pass | pass | pass | pass |
| Registry change during a run leaves runs in flight alone (new) | n/a (a changed value is a new value; asserted: original keeps 3 tools, clone has 4) | | | | pass | **fail** (`add` returned 100 instead of 5) |
| Parallel runs on one held registry object (new) | | | | | | R2 in Wasm: **fail** (run B waited 151 ms for run A) |
| Blocking tool beside two async siblings (300 ms vs 2 x 50 ms), blocking tool first | K1 **fail** (stall 300 ms) / K2 pass (0 ms) / K3 pass (0 ms) | B2 as K1: **fail** (stall 301 ms) | pass (stall 1 ms) | pass (1 ms) | | |
| Host main thread stays responsive while a blocking tool runs (run on the main thread, 5 ms heartbeat beside it) | K1 **fail** (300 ms block) / K2 pass (0 ms) / K3 pass (0 ms) | B2 as K1: **fail** (300 ms) | pass (0 to 1 ms) | pass (0 to 1 ms) | | |

Wasm-only checks: the guest imports 14 functions (5 start forms, 9 shared) and exports 5 (`memory`, `guest_alloc`, `guest_register`, `guest_run`, `guest_out_ptr`); a gated-core source scan test and a mutation check (removing the body's close-on-drop makes `dropping_the_body_closes_the_socket_while_the_link_lives` fail) are in `cb-host/tests/gate.rs`.

## Measurements

Timings are noisy (shared 2-core machine, other builds ran during some runs, wasmtime is a debug build). Latencies are medians of 7 (min to max within 0 to 3 ms) unless stated.

### Crossings and bytes per run (B2, B3)

A crossing is one call over the boundary in either direction. B3 counts every import and export call; "payload" leaves out the bookkeeping calls (`op_poll`, `wait`, `cancelled`), whose number depends on timing (see Surprises). B1 crosses nothing serialised.

| Run | B2 crossings / bytes | B3 typed total (payload) / bytes | B3 generic total (payload) / bytes |
|---|---|---|---|
| typed result + 1 tool (2 model calls, 1 tool, 1 validate) | 17 / 1747 B | 32 (19) / 2933 B | 26 (19) / 2948 B |
| model reply streamed, 6 deltas | 19 / 950 B | 83 (9) / 1755 B | 83 (9) / 1761 B |
| Transport call, 8-word streamed body | 27 / 463 B | 141 (29) / 656 B | 141 (29) / 738 B |
| Transport call cancelled after 3 deltas | 13 / 242 B | 61 (16) / 365 B | 55 (16) / 411 B |
| five tool calls in one turn | 33 / 3103 B | 52 (35) / 4781 B | 49 (35) / 4812 B |

- Where the bytes go: the model request (history plus tool specs) is 505 to 1173 bytes per call and re-sent whole each call (grows with turns, as in ticket 35); a tool call is about 50 bytes out and 40 to 70 back; one delta is 18 to 20 bytes as `{"Delta":"w3 "}`.
- A delta costs 2 crossings in B2 (the host's model calls `emit` into the library, the library forwards to the host sink: `plain:emit` 6 + `wire:emit` 8, of which 2 are core events) and 0 library crossings in B3 (the model callback and the sink are both host side, so the delta never enters the guest).
- A body chunk costs 2 crossings (`body_next` out, chunk back) in B2; in B3 2 payload crossings (`body_next_start`, `op_take`) plus polling.
- State crosses once per run in B3 (556 B in, 627 B out for the typed-result run) because the Loop runs inside the guest; it does not cross at all in B2 (the Loop is library side). A host-owned loop over a Wasm core would cross State on every step (not built, not measured).
- Generic vs typed: the generic import adds the operation name to each start call (+4 to +9 bytes per call, +15 to +82 bytes per run above) and removes 4 of 14 imports (13 used vs 10).
- Body chunks are raw bytes in B2 and B3. Forcing them through JSON would need base64 (+33 %, no `std` needed: `base64` supports `no_std`), or a number array (about 3 to 4 times).

### Wasm instance costs (B3)

| What | Value |
|---|---|
| guest module (release, `opt-level = "s"`, lto) | 230 380 bytes |
| compile once (wasmtime 50.0.1 debug build, cranelift) | 3.1 to 3.2 s (release build of wasmtime not measured) |
| instantiate per run | 0.23 to 0.5 ms |
| linear memory after a small run | 1152 KiB per instance (18 pages) |
| host threads | one thread per concurrent run (the guest blocks it for the whole run) plus a 4-thread pool for the host's callbacks |
| dependency tree | `cb-core`: 12 external crates (serde, serde_json, spin and their build-time companions); `cb-host`: 123 packages (wasmtime) |

### Abort and cancel latency, median of 7 (ms)

| | B1 | B2 | B3 typed | B3 generic |
|---|---|---|---|---|
| abort until the hung tool's future is dropped | 0 | 0 | 0 | 0 |
| abort until a hook ran in the callback | 0 (7/7) | never (0/7: no hook in `Api`) | never | never |
| parent abort until the inline child's hung tool is freed | 0 | 0 (the child is dropped with the tool future) | 1 | 1 |
| parent abort until the host-spawned child is freed | 0 | 2 (polling thread, 2 ms interval) | 1 | 1 |
| inline child reports `Aborted` to its parent's tool | 7/7 | 0/7 | 0/7 | 0/7 |
| cancel until the loopback server sees the hang-up | 0 to 1 | 0 to 1 | 0 to 1 | 0 to 1 |
| first streamed delta (model emits every 20 ms) | 20 | 20 | 21 | 21 |

With the host's own run token handed to callbacks (host side only, nothing crosses; `Rig::with_host_token`), 20 runs each: B2 inline child reports `Aborted` 20/20, B3 1/20; host-spawned 20/20 both.

### K axis: sibling stall and main-thread block (B1 baseline, median of 5)

Blocking tool: `std::thread::sleep(300 ms)` standing in for a synchronous file read or any blocking syscall; first in a batch of three; the two siblings each wait 50 ms on a host timer. Config M: the host drives the run on its main thread (single-threaded `LocalPool`) with a 5 ms heartbeat beside it. Config P: the run is spawned on a 2-thread pool and the main thread only waits.

| Variant | sibling stall, M | main-thread block, M | run time, M | sibling stall, P | main-thread block, P |
|---|---|---|---|---|---|
| K1 | 300 ms | 300 ms | 366 ms | 300 ms | 0 ms |
| K2 (runner = `std::thread::spawn`) | 0 ms | 0 ms | 315 ms | 0 ms | 0 ms |
| K3 (runner = `thread-runner`, 4 workers) | 0 ms | 0 ms | 315 ms | 0 ms | 0 ms |
| K2, runner = tokio `spawn_blocking`, current-thread runtime | stall 0 ms, heartbeat block 1 ms | | | | |

Across boundaries, the same blocking tool written as an async host callback that blocks (config M): B1 stall 300 ms / block 300 ms; B2 301 / 300; B3 typed 1 / 0 to 1; B3 generic 1 / 0 to 1. In B3 every host callback is started by the host on its own pool, so a blocking one cannot stall the guest's other calls.

K3 capacity: two 100 ms blocking tools and one async sibling: 1 worker finishes the batch in 200 ms, 2 workers in 100 ms; the async sibling is 50 ms either way.

### K axis: does abort release a blocked tool? (abort 50 ms into a 300 ms block, median of 5)

| Variant / blocking body | run ends after abort | blocked thread free after abort |
|---|---|---|
| K1 / uncooperative (`sleep`) | 250 ms | 250 ms |
| K1 / polls the token every 2 ms | 1 ms | 0 to 1 ms |
| K1 / token hook (condvar wake) | 0 ms | 0 ms |
| K2 / uncooperative | 0 ms | 250 ms |
| K2 / polls the token | 0 ms | 0 to 1 ms |
| K2 / token hook | 0 ms | 0 ms |
| K3 / uncooperative | 0 ms | 250 ms |
| K3 / polls the token | 0 ms | 1 ms |
| K3 / token hook | 0 ms | 0 ms |

K3 with 1 worker: after run A is aborted at 50 ms with an uncooperative 400 ms tool, run B's 10 ms blocking tool took 360 ms (it waited for A's worker).

### R axis

- R1: 100 000 clones of a 3-tool capability value took 2.6 ms (one `Arc` increment each); the handle count returns to 1 after a run (`Caps1::holders`); a changed value is a new value (original keeps 3 tools, the changed clone has 4).
- R2: two contexts are independent; `cb-core/src` contains no `static`, `thread_local` or lazy global (asserted by a source scan).
- R2 snapshot vs live, one run in flight while the host re-registers `add` and registers `extra` through the context: snapshot run saw `add -> 5`, `extra -> "unknown tool extra"`; live run saw `add -> 100`, `extra -> "unknown tool extra"`. A run started afterwards sees `100` in both.
- R2 in Wasm: a second run on the same held instance waited 151 ms for the first; two instances (R1 in Wasm, one per run) ran in parallel.

### The gate

| Piece | `cargo check --target wasm32v1-none` |
|---|---|
| `cb-core` (gated, all of link, b1, b2, context, abi) | passes; `forbid(unsafe_code)`, no `std::`, no thread, no spawn, no sleep, no runtime named (source scan test) |
| `call-core` (reused; run in its own workspace) | passes |
| `thread-runner` (K3, std, `gated = false`) | fails: "can't find crate for `std`" (by design, it is outside the gate) |
| `cb-guest` | built for `wasm32v1-none` as a cdylib (release); not a gated crate (allocator and exports need `unsafe`) |
| wasmtime | dependency of `cb-host` only; not in any gated crate's tree (`cargo tree -p cb-core` lists 12 external crates, none of them wasmtime) |

Not done from ticket 35's gate list: cargo-deny allowlist, the clippy `await_holding_invalid_type` rule for `spin`, the zero-import probe. By inspection no `spin` guard is held across an `.await` in `cb-core` (`Shot` takes the lock inside `poll`, `Context` inside plain methods).

## Validated

**Boundary.** Variants that pass every scenario: B1, B2, B3 typed, B3 generic, with one qualification on scenario 5c below. Rule that fits all passing variants: a callback needs only owned serialisable data plus plain callbacks standing in for handles (`cancelled()`, `emit(bytes)`, and a body read and close addressed by id); a handle type (token with hooks, sink, lent body stream) can exist only as an in-process convenience over those, and nothing in the scenarios requires one. Failures, each with a named reason, all on scenario 5c (an inline child reports `Aborted` back to the tool that started it; 0/7 in B2, B3 typed, B3 generic): (B2, B3) `cancelled()` is poll-only, so a child's token is cancelled by a polling thread after the parent's extra poll has already passed and the child is dropped before it can report; (B3) the extra-poll guarantee cannot cross the boundary at all, because the guest's extra poll is an `op_poll` and the host's tool future is aborted by `op_drop` regardless (1/20 even with the host's token). The same scenario passes in B1 (7/7) and for a host-spawned child in every variant (7/7).

**Registry.** Variants that pass every scenario: R1 and R2 snapshot (both also pass the generic `call(name, args)` import in B3: 11 scenarios unchanged, +15 to +82 bytes per run). Rule that fits all passing variants: the capability set is a value copied into each run at its start, whether the host builds it per call or a context object hands out a snapshot, and a run never looks callbacks up in anything that can change under it. Failures: R2 live (name: a registration through the context changed a run already in flight, so runs share a registry, against ADR 0004; and a tool added mid-run stays invisible to the model because the spec was fixed at start, so it is also inconsistent); R2 in Wasm (name: one guest instance runs one thing at a time, so parallel runs on one held context serialise, 151 ms stall; the fix is an instance per run, which makes the held object host-side and the registry an R1 value).

**Blocking.** Variants that pass every scenario: K2 and K3 (also K2 with tokio's `spawn_blocking` as the runner, and the Wasm boundary, where host-started callbacks cannot stall the guest). Rule that fits all passing variants: a blocking callback is registered through a blocking form and runs on a thread the host supplies through a runner capability (written by the host or shipped in an opt-in std crate), so the run's task and the gated core never execute blocking code or start a thread, and `tool_blocking` returns a typed error if no runner is registered yet. Failure: K1 (name: blocking inside an async callback stalls every sibling polled in the same task for the whole blocking time, 300 ms of 300 ms, and with the run on the host's one thread it blocks the host for the same time, 300 ms; the contract "must not block" cannot be checked by the library; an uncooperative body also delays abort by the rest of its blocking time, 250 ms). Neither K2 nor K3 frees an uncooperative blocked thread on abort (250 ms), and a bounded runner stays occupied by it (K3 with 1 worker: the next blocking tool waited 360 ms).

## Surprises

1. **The synchronous cancel hook does not survive the plain form, and the "extra poll" guarantee does not cross Wasm.** Ticket 35's contract (token with sync hooks, extra poll, cancel wins over a late result) is what lets an inline child report `Aborted`. With `cancelled()` only, that report is lost (0/7); handing the callback the host's own token fixes it in B2 (20/20) but not in B3 (1/20), where the guest's extra poll is just a poll of an operation the host aborts anyway. A cooperative `op_drop` (flag first, abort after a grace period) would likely fix B3; not built.
2. **A host-held token is not a handle that crosses.** When the abort originates in the host (`token.cancel()`), a host callback can use the host's own token in every boundary; only a library-originated cancel (a deadline or budget in a shipped loop) needs `cancelled()` or a drop to reach a callback. The scenarios only abort from the host, so this split is not exercised for library-originated cancel.
3. **Wasm cancel is a flag plus a wake, and the guest has no wakers.** The host cannot call into a guest blocked in `wait`, so abort is an atomic flag plus a condvar bump; the guest re-polls every outstanding operation after each wake (`op_poll` 43 times for an 8-chunk body). I gave `wait` a 5 ms timeout so flags set by a linked parent are noticed; that costs 3 bookkeeping crossings (`cancelled`, `op_poll`, `wait`) every 5 ms per run, about 24 per 40 ms (83 crossings for a 120 ms run with 9 payload). The timeout is also the cancel latency for polled flags. A design could drop the timeout when no linked child exists.
4. **The sink does not need to cross at all in Wasm.** A model callback emitting deltas and the host sink are both host side, so in B3 a delta costs 0 library crossings against 2 in B2. A handle that points back at the host needs no crossing.
5. **B2 behaves as K1.** A blocking host callback in B2 stalls siblings and the main thread (300 ms) exactly like K1, because the library polls host callbacks in its own task; B3 is immune (stall 1 ms) because the host starts every callback on its own pool. The blocking question is a property of who polls the callback, not of serialisation.
6. **K2 is order dependent.** `tool_blocking` returns `NoBlockingRunner` if the runner is registered after it; a runner registered later does not rescue it. Checking at registration, as the ticket says, forces "runner first".
7. **A bounded runner can be starved by an aborted tool.** The run ends at once, but the uncooperative thread stays blocked (250 ms here; forever for a hung syscall), and with one worker the next blocking tool waited 360 ms.
8. **The lent body worked in all three boundaries.** `BodyStream::next(&mut self) -> Future + '_` is usable in B1 (the host's `SockBody`), over the byte wire in B2 (the library holds an id, the host holds the body) and over Wasm imports in B3. Only B1 passes a body object across; B2 and B3 pass an id. That answers the "body is lent to the future" carry from ADR 0005 for the three shapes tried (no real HTTP client was used; the body is a thread-fed channel over a std `TcpStream`).
9. **State and the loop's location decide what crosses.** With the Loop inside the guest, State crosses twice per run (about 0.6 KB each way here), not per call; with a host-owned loop over a guest core it would cross every step (not built).
10. **The gate only works per workspace member.** `cargo check -p call-core --target wasm32v1-none` from this workspace fails with 5829 errors (serde's `std` feature, unified in from `cb-host`'s default-feature serde, as in #27); `-p cb-core` passes in the same workspace, and `call-core` passes in its own. Each gated crate needs to be a member of a workspace that has no std-feature dependants, or be checked in its own.
11. **The guest built first try.** `no_std` + alloc + `dlmalloc` (feature `global`) + `wasm32v1-none` cdylib with 14 imports, 230 KB; all `unsafe` is in the 143-line guest wrapper (allocator, import declarations, slice reads at the export edge), none in `cb-core`.
12. **A test that cannot fail was found and fixed.** The first "drop the body" scenario passed with the close removed, because dropping the in-flight read, or ending the run, closed the socket anyway. `dropping_the_body_closes_the_socket_while_the_link_lives` reads one chunk, then drops the body with the link still alive; it fails when the close is removed.

## Limits and not done

- Loopback mocks only: a scripted model, a TCP listener on 127.0.0.1 streaming SSE-like lines (chunks split mid-line), no TLS, no real HTTP parsing beyond the status line.
- The Exchange here is a stand-in (reads `data: {"t":...}` lines), not the OpenRouter codec; the claim is about the boundary, not the wire format.
- Timings: single shared machine, debug builds, medians of 5 to 7, bookkeeping crossings vary run to run (23 to 32 for the 17-crossing run). Wasm compile time is for a debug build of wasmtime.
- Not built: cooperative `op_drop`; a host-owned loop over a guest core; library-originated cancel (deadline) across the boundary; a `Send`-less (`?Send`) variant; structured errors (errors cross as strings here); base64 body encoding; a real Wasm component (WASI/component model): the guest is a core module with hand-written imports.
- The ticket 35 scenarios for fan-out and depth limits, checkpoint/resume and the fake-clock budget were not re-run (they exercise the host's own loop and policy, not the callback boundary); the pure core's 17 tests in `single-call-core` still stand.
- Blocking is simulated with `thread::sleep`; a real synchronous file read behaves the same for the stall metrics.
