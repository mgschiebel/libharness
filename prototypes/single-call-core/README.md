# PROTOTYPE (throwaway): single-call core with host-owned, injected and shipped loops

Ticket: mgschiebel/libharness#35. Branch `prototype/single-call-core` (based on `prototype/session-core`), never merged to main.

**Question:** the library's core capability is one model call, with state passed in and returned per call, and the loop is a host choice (host-owned code, a loop registered as a callback, or an optional shipped loop). Does that layering work, and what does each loop owner cost?

**Answer in one paragraph:** yes. The core is a pure `(State, input) -> Out` function set with three transitions (`absorb` a model reply, `tools_done` a tool batch, `abort`), plus `begin`/`resume` and the `steer`/`follow_up` queue operations. `State` is plain serialisable data, 237 bytes for a fresh run and about 174 bytes more per tool turn. The model, every tool and the typed-result validator are host callbacks. The same nine session-task scenarios (plus one new checkpoint/resume scenario) pass under all three loop owners: the host's own plain loop with no executor at all (11 tests), a host-written loop registered as a callback, and the shipped loop, the last two on tokio, futures-executor `ThreadPool` and `LocalPool` (57 tests, 0 failures in 40 repeated runs). No runtime types appear in any library API. The `call-core`, `session-shell` and `shipped-loop` crates build `no_std` + `alloc` for `wasm32v1-none`. The translation cost measured in #26 did not move to the shipped loop; it vanished (shipped loop: 44 lines, 0 mapping lines), because the core was written in the session's own types. What is left is about 90 lines of API mapping in the shell, paid by the injected and the shipped owner alike (not by host-owned code). Details and the places where the hypothesis does not hold as worded are below.

## Run

```
cd prototypes/single-call-core
cargo test --workspace                          # everything (87 tests)
cargo test -p call-core                         # core alone: 17 pure tests, no executor, no futures crate, no clock
cargo test -p call-core --test boundary_size -- --nocapture   # what crosses the boundary, in bytes
cargo test -p host-owned                        # OWNER 1: plain code, no executor (11 tests)
cargo test -p host-tests                        # OWNERS 2 and 3 on tokio + futures-executor (57 tests)
cargo check -p call-core --target wasm32v1-none # the ADR 0002 gate (also session-shell, shipped-loop)
cargo tree -p host-owned                        # call-core + serde_json only: no futures, no tokio
```

`wasm32v1-none` was already installed (`rustup target list --installed`); nothing had to be fetched.

## What was built

| Crate | Role | `no_std` + alloc | Lines (`wc -l`) |
|---|---|---|---|
| `call-core/src/lib.rs` + `step.rs` | the pure core: types, `State`, transitions | yes | 174 + 218 = **392** |
| `call-core/src/cancel.rs` | `CancelToken` with sync hooks (session-task's, `Mutex` -> `spin::Mutex`) | yes | 92 |
| `call-core/src/registry.rs` | `Callbacks`: model, tools, validator as boxed `Send` callbacks | yes | 76 |
| `call-core/src/aio.rs` | optional async helpers: `abortable` (extra poll), `join_all`, `call_model`, `call_tools` | yes | 81 |
| `session-shell` | handle, inbox, typed `R` edge, `on_result`, the loop-registration hook (`LoopFn`), `Session::start`/`resume` | yes | 221 |
| `shipped-loop` | OWNER 3: session-task's loop rebuilt on the core | yes | 44 |
| `host-owned` | OWNER 1: the host's own plain loop, with its tests | no (host code) | 57 (+ 319 tests) |
| `host-tests/src/injected.rs` | OWNER 2: a loop the host wrote and registered with `caps.with_loop(..)` | no (host code) | 34 |
| `host-tests` rest | scenarios and fixtures reused from `prototype/session-task` (ours), run on tokio and futures-executor | no | 155 + 401 + 66 |

### The core's one operation

```
begin / resume(state, spec)                          -> Out { state, step, events }
absorb(state, spec, validate, model_result)          -> Out        // one model call, absorbed
tools_done(state, spec, Vec<ToolDone>)               -> Out
abort(state)                                         -> Out
steer(state, text) / follow_up(state, text)          -> State      // queues live in the state

Step::Call(ModelRequest) | Step::Tools(Vec<ToolCall>) | Step::End(Result<Value, SessionError>)
```

`absorb` is where the "single model call" is interpreted: it appends the assistant message, validates `final_answer` against the host's validator (typed result, bounded retry, rejection fed back as a tool result), answers unknown tools itself, nudges a silent model once, honours steer (all queued steers at the next request) and follow-up (only when the run would stop), counts turns. `tools_done` matches results to calls by id, emits them in source order, and applies the stop rule (accepted answer ends the run unless input is queued). A loop is therefore three `match` arms (see `host-tests/src/injected.rs`). Nothing is kept between calls: every function takes `State` by value and returns it.

The async convenience `Callbacks::call_model(req, &token)` is the same single call with the host's registered callback invoked; it returns the raw reply (or `None` if aborted) so the loop owner chooses what to do with state before feeding the reply to `absorb`.

### The three loop owners

1. **Host-owned plain code** (`host-owned/src/lib.rs`, 57 lines, 48 code lines): `run(&mut Wiring, state)`, blocking `FnMut` callbacks, a mailbox closure for steer/follow-up, a cancel token check, and a per-step hook (used for checkpoints and a fake-clock budget). No `async`, no futures, no registry. The smallest useful form (typed result plus tools, no steer, no abort) is **13 lines** (`minimal` in `host-owned/tests/scenarios.rs`, runs the typed-result scenario).
2. **Injected loop** (`host-tests/src/injected.rs`, 34 lines, 26 code lines): `Arc<dyn Fn(LoopCtx, State) -> BoxFut<(State, Result<Value, SessionError>)>>`, registered with `caps.with_loop(host_loop(..))`. State in, state out; the `LoopCtx` hands it the host's callbacks, the cancel token and the inbox. This one also records the serialised state before every step (checkpoint).
3. **Shipped loop** (`shipped-loop/src/lib.rs`, 44 lines, 38 code lines): the same type, library-written, forwards events and checks the token at every boundary (like session-task's turn top). Host writes one line: `caps.with_loop(shipped())`.

Owners 2 and 3 are the same mechanism: a shipped loop is just a library-provided callback of the loop type. The difference is who wrote the 20-40 lines.

## Scenario x loop-owner results

All pass. "Pass" for the async owners means on tokio (4-thread), futures-executor `ThreadPool` and `LocalPool`. Owner 1 runs with no executor.

| Scenario | 1 host-owned (no executor) | 2 injected loop | 3 shipped loop |
|---|---|---|---|
| typed result via `final_answer` + a tool | pass | pass (x3 executors) | pass (x3) |
| malformed answers: retry, `Malformed`, nudge then `NoAnswer`, `R = String` | pass | pass (x3) | pass (x3) |
| steer lands at the next boundary; follow-up only when stopping | pass (input pushed from inside the fake model, standing in for a thread) | pass (x3) | pass (x3) |
| abort while a tool hangs: hook runs, result is `Aborted` | pass (cooperative blocking tool released by the hook; abort delivered from a test thread) | pass (x3) | pass (x3) |
| spawned child through an ordinary tool callback, host depth limit | pass (the tool calls the host's own `session()` function, recursion) | pass (x3) | pass (x3) |
| fan-out limit | pass, **different semantics**: sequential plain code, so a per-session budget, not a concurrency cap | pass (x3) | pass (x3) |
| cancel reaches the child (host-spawned) | not run (a plain loop has no executor to spawn on; the child is inline) | pass (x3) | pass (x3) |
| cancel reaches the child (inline, the "extra poll" case) | pass (same test as the line above for owner 1: the child is a function call inside the tool) | pass (x3) | pass (x3) |
| two sessions in parallel, no shared state | not run (two `State` values are independent by construction; nothing static) | pass (x3) | pass (x3) |
| checkpoint, then resume in a fresh run from stored bytes only (new) | pass | pass (x3) | not applicable (the shipped loop does not checkpoint; the shell's `Session::resume` accepts any `State`, so it could resume) |
| time budget with a fake clock (new) | pass (step hook reads the host's fake clock; 0.00 s) | not built | not built |

Test counts: `call-core` 17 pure + 1 source scan + 1 size table; `host-owned` 11; `host-tests` 19 on tokio (9 shipped + 10 injected) and 38 on futures-executor (the same 19 on `ThreadPool` and `LocalPool`). 40 consecutive runs of `host-tests` and `host-owned`, 0 runs with a failure. **Timings are noisy**: three other builds were running on this 2-core machine, so treat any wall time as an upper bound (every suite finished in 0.00 to 0.03 s regardless).

## Measurements

### Lines

| What | Lines (`wc -l`, blanks and comments included; code-only in brackets) |
|---|---|
| Core, pure layer (types + transitions, no async, no callbacks) | **392** (326) |
| Core, everything (pure + cancel 92 + registry 76 + async helpers 81) | **641** (537) |
| Shell (handle, inbox, loop hook, typed edge, registration API) | 221 (190) |
| Shipped loop | 44 (38) |
| Host-owned loop, as used by the scenarios (steer, abort, step hook) | 57 (48) |
| Host-owned loop, minimal (typed result + tools) | 13 |
| Injected loop (with checkpointing) | 34 (26) |
| Everything on the shipped path: core + shell + shipped loop | 906 |
| For comparison: session-task library 513; session-core library 719 + the 432-line sans-IO core = 1151 | |

### Lines a host must write, per case

| Case | Host writes | Library lines it depends on |
|---|---|---|
| 1 host-owned | its loop: 13 minimal, 57 with steer + abort + step hook; plus its model/tool/validator functions (needed in any case); a mailbox and a token if it wants steer/abort | 392 pure core (+ 92 token if it hands tokens to callbacks); no shell, no async |
| 2 injected | its loop: 22 minimal, 34 with checkpointing; plus the same registrations as in session-task (`Capabilities::new().model().tool().on_result()`) | 641 core + 221 shell = 862 |
| 3 shipped | 1 line (`.with_loop(shipped())`) plus the same registrations | 906 |

What the host writes for registration (model, tools, `on_result`) is the same in owners 2 and 3 as in the session-task prototype. The scenario files needed 89 changed lines against `prototype/session-task` (36 of them the new checkpoint scenario; the rest mechanical: the model callback now takes the token as a second argument, `.with_loop(..)`, one `Step` -> `Turn` rename in the fixtures).

### Is the core testable with no executor and a fake clock?

- **No executor: yes.** `cargo test -p call-core`: 17 plain `#[test]`s in 0.00 s, no `async`, no futures crate in the manifest, no threads. They cover typed result, malformed retry and exhaustion, nudge then `NoAnswer`, steer at the boundary (all steers together, and one sent before the first call), follow-up only when stopping, a queued steer keeping the run going after an accepted answer, abort from every phase (idempotent), stale inputs as a typed `Protocol` error, `max_turns`, unknown tool and source-ordered results with completion-order input, model failure, a JSON round trip of the state after every step, and `resume` from a serialised state in each phase. A source scan test (`no_runtime.rs`) checks the core for `tokio`, `futures`, `std::`, `thread`, `spawn`, `sleep`, `Instant`.
- **Fake clock: not applicable inside the core, usable around it.** The core takes no time at all: no `now` argument, no deadline (the #26 core had both). Timeouts and budgets are the loop owner's. `a_wall_clock_budget_with_a_fake_clock_and_no_timers` (host-owned) enforces a 100 ms budget from the step hook against a fake clock: deterministic, no sleeping. A real per-call timeout (interrupting a call in flight) needs a timer, which stays the host's, as decided.

### What crosses the boundary per call

`cargo test -p call-core --test boundary_size -- --nocapture` (sample: prompt, five `add` tool turns, then `final_answer`; JSON bytes via `serde_json`):

| Step | State bytes | Model request bytes | Messages in state |
|---|---|---|---|
| begin (prompt only) | 237 | 643 | 1 |
| after tool results, turn 0 | 411 | 817 | 3 |
| after tool results, turn 2 | 759 | 1165 | 7 |
| after tool results, turn 4 | 1107 | 1513 | 11 |
| after `final_answer` (End) | 1308 | n/a | 13 |

- Shape: `State` is `{ id, cfg, phase, messages[], steer[], follow[], pending[], answer, turn, rejected, nudges }`; `messages` is the whole history and is the only part that grows. Growth is about 174 bytes per tool turn (one assistant message and one tool result). An empty state is 193 bytes, 54 of them the config.
- The model callback gets a `ModelRequest` (history + tool specs, 643 bytes at the start; the spec is 409 bytes of that and is the same on every call). The core re-sends the whole history on every call, as every chat API does, so the total handed to the model over this 6-call run is 6468 bytes (quadratic in turns). That is a property of the request, not of the layering.
- Not in the state: the callbacks, the tool specs and result schema (`Spec`, read-only data kept in the registry), the cancel token, the inbox. A loop owner that persists state persists `State` only.
- Resume re-asks the call that was in flight when the state was taken (`resume` rebuilds the pending `Step` from the phase tag and the unanswered `pending` calls). A model call is not idempotent, so a resumed run may pay for one call twice.

### Where cancel and the "poll once more after cancel" contract land

| Piece | Lands in | Lines |
|---|---|---|
| `CancelToken` (flag, wakers, sync hooks) | core crate, `cancel.rs`; `no_std` through `spin::Mutex` (never held across an await) | 92 |
| The extra poll (`abortable`: results win the poll in which the token fires) | core crate, `aio.rs`; reached only through `call_model` / `call_tools` | ~20 |
| **Cancel wins over a result that landed in the extra poll** (`settled`) | core crate, `aio.rs` | 4 |
| `abort(state)` transition | pure layer, `step.rs`; the state machine knows no token | 3 |
| Boundary check before each step | owner 3 had it (session-task's turn top); now redundant (see Surprises); owner 1 keeps the sync equivalent | 3 |

- **Owners 2 and 3 pay nothing** for cancel if they await through `call_model` / `call_tools`; they only map `None` to `abort(state)`. An owner that awaits its own futures must wrap them in `abortable` and apply `settled` itself.
- **Owner 1 needs a different contract.** A sync callback is never dropped, so the "extra poll" is vacuous: the host's blocking tool simply returns after the hook fired. What remains is the rule "check the token after every callback and before every step" (3 lines in `host-owned::run`). The hook is what releases a blocked tool; without cooperation a plain loop cannot interrupt a call (see "Not expressible").
- Mutation check: with the extra poll removed from `abortable`, `cancel_inline_child` never finishes for both async owners (2 of the 19 tokio tests hang; the other 17 pass). So the contract matters only for the inline child, as in session-task.

### `no_std` + `alloc` (ADR 0002)

`cargo check -p call-core --target wasm32v1-none`, and the same for `session-shell` and `shipped-loop`: all three pass (first try). `forbid(unsafe_code)` everywhere; each declares `[package.metadata] gated = true`. Dependencies of `call-core`: serde (alloc), serde_json (alloc), spin 0.9, plus serde's proc-macro crates (15 crates in the tree including itself; session-task had 14: `spin` is the only addition). `host-owned` depends on `call-core` and `serde_json` only. Not done: cargo-deny allowlist, the clippy `await_holding_invalid_type` rule for `spin::MutexGuard`, the zero-import probe build. By inspection no spin guard is held across an `.await`.

## Hypothesis verdict

**Hypothesis:** the sans-IO core's "state in, outputs out" shape is the lower layer, so the translation cost measured in #26 (about 300 lines) applies only to the optional shipped loop.

**Verdict: the layering holds, but the hypothesis as worded is wrong in two ways.**

1. **The shape is the right lower layer, and its cost is near zero when the core is written in the session's types.** Owners 1 and 2 use the core's types natively: 0 translation lines. The shipped loop, the one place the hypothesis expects the 300 lines, is 44 lines with 0 mapping lines. The #26 cost was a retrofit cost (string `args`, no tool name on results, no typed result, events to remap, no "answer ends the run"), not a cost of the shape. Both #26 divergences (several queued steers, a steer before the first poll) and the core patch for "stop after the answer" disappear with a core written for this.
2. **The old core could not have been the lower layer.** `sans-io-core` (432 lines) fails `cargo check --target wasm32v1-none` with 79 errors (it uses `std::time::Instant` and `std` types, takes `&mut self`, and none of its types is serialisable), so "state in as owned serialisable data" was a rewrite, not a re-layering. The new pure layer is 392 lines including all the session policy that #26 put in a 249-line engine on top of a 432-line core (681 lines there). It is smaller partly because it dropped things the old core had: deadlines, per-event model streaming, a `Busy` error. Streaming stays unspecified, as decided.
3. **The residual translation cost is not "the shipped loop's"; it is the shell's.** About 90 of the shell's 221 lines map between the core's data and the session-task API shape (typed `R` at the edge, the `Capabilities<R>` delegation to `Callbacks`, `SessionConfig`, `SessionEnd`). Owners 2 and 3 both pay it; owner 1 does not, and in exchange has no handle, no `on_result`, and writes its own mailbox and cancel check (the 44 extra lines between the 13-line minimal loop and the 57-line one).

## What could not be expressed, and why

| Behaviour | Result |
|---|---|
| Aborting an uncooperative blocking call from a single-threaded plain loop (owner 1) | **Not expressible without help.** Nothing can preempt a blocking function on the same thread. Abort worked because the hook released the cooperative blocking tool, and the abort was delivered from another thread (a test thread). A host calling a blocking HTTP client needs a client that supports cancel, or a thread. |
| Concurrent tool batch in owner 1 | The core permits it (results are matched by call id, any order); the plain loop ran the batch sequentially because concurrency needs threads or an executor. Hence the fan-out difference above. |
| Steer or abort arriving while a loop owns the state | The state is owned data held by the loop, so an outside party cannot touch it. A shared inbox and token are unavoidable; they live in the shell (owners 2, 3) or in the host's own variables (owner 1). |
| A wall-clock limit on a call in flight, retries with back-off | Need a timer; none in the library (decided). Per-step budgets work (fake-clock test). |
| JSON-schema validation of the typed result inside the core | Not in a `no_std` core without a validator dependency; the host supplies a sync validator callback (`serde_json::from_value::<R>` in the typed helper). |
| Streaming | Left unspecified (decided); a model reply is one value. |

## Surprises

1. **The cancel contract is not fully inside "token + extra poll".** The first cut of the injected loop (no boundary check) failed 4 tests (`cancel_inline_child` and `cancel_reaches_spawned_child`, on both futures-executor pools): in the extra poll the aborted tool batch completed with `child failed: Aborted`, `abortable` returned that result ("results win"), and the loop carried on to a second model call and finished `Ok`. The shipped loop passed only because it carried session-task's turn-top `is_cancelled` check. The rule "cancel wins over a result that arrives in the extra poll" had to move into the helpers (`settled`, 4 lines); then the shipped loop's check became redundant (removed it temporarily: 57 of 57 still pass). So every loop owner needs either the core's helpers or its own three-part discipline (token, extra poll, settle).
2. **The old sans-IO core could not be the lower layer at all** (no `no_std`, not serialisable); see verdict point 2. I expected a "compose with a patch" result like #26 and got a rewrite.
3. **The 300 lines of translation did not move, they disappeared** (verdict point 1). I expected them to reappear in the shipped loop. What remains is API-shape mapping in the shell, shared by two owners.
4. **The shell is the biggest piece after the core** (221 lines, more than the 218-line transitions file) and it exists only because an outside party must steer, abort or receive a typed result from a loop that owns the state. Owner 1 shows the layers without it. That is a datum for the open question whether "session" is a library concept; it is not decided here.
5. **Owner 1 needs no executor, but its abort still needs a second thread** (to deliver the abort in the test) and cooperative callbacks. "No executor at all" is true of the loop and false of the concurrency a host may want around it.
6. **`resume` came free.** A phase tag plus the list of pending calls was enough to rebuild the next step from stored bytes; `Session::resume` and the checkpoint scenario needed no core change beyond one 15-line function.

Confirming expectations, not surprises: `no_std` + `alloc` compiled on the first try (serde_json with only `alloc`, `spin` for the token); the scripted pure tests passed first try and ran in 0.00 s; the executor-neutral scenarios passed unchanged in shape on tokio and futures-executor; spawning needed no library concept in any owner (the child is an ordinary tool callback; for owner 1 it is a recursive function call; the token's sync hook is the only library support it wanted); `Send` still leaks into the host (the `LocalPool` run still needs the channel forwarder); history is re-sent whole on every call so request bytes grow quadratically.

## Notes for the open questions (not decided here)

- **"Session" as a library concept (#21):** with owner 1 there is no session object, only a `State` value and the host's loop. A "session" shows up exactly where the shell is needed: handle, inbox, typed `on_result`. Whether the library should ship that shell is the question; this prototype only shows its size (221 lines, about 90 of them API mapping).
- **Continuity:** state-in/state-out makes continuity a data question (store the JSON, call `resume`); the cost is re-issuing the in-flight call. Reasoning blocks as history (a carried-forward fog item) would live in `Message`; not tested.
- **Spawning:** unchanged by the layering. A spawned child is an ordinary tool callback in all three owners; cancel reaches it through the token hook; parent/child ids, cost roll-up and inheritance stay host-side.
- **For the recommendation (#20):** the layering works and costs little. Owner 1 is a 13-line loop over a 392-line pure core and needs no executor; the injected and shipped loops are the same mechanism (a callback of type `LoopFn`), so a shipped loop is an optional 44-line library callback, not a special case of the library.

## Assumptions made (AFK)

- Rebuilt the core instead of patching the #26 one (it cannot meet the ticket's "serialisable state in/out" or the `no_std` gate); reused our own earlier code: session-task's cancel token, `abortable`, `join_all`, turn semantics, scenarios and glue. No code from other projects.
- The validator for the typed result is a sync host callback; the host chooses `R` (`Capabilities::<R>::new(schema)`) and supplies the JSON schema.
- Tool specs and the result schema are `Spec`, read-only data next to the callbacks, not part of `State`.
- `State` fields are `pub` for the prototype; only the phase check guards misuse (`SessionError::Protocol`).
- Owner 1's "steer arrives in flight" is simulated by the fake model pushing into the host's mailbox; abort and the cancelled child use `std::thread` in the tests only.
- One toy with scripted fakes, no streaming, no real timers, no persistence beyond in-memory JSON strings; line counts include blanks and comments.
