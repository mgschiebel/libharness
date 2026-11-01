# PROTOTYPE (throwaway): sans-IO core inside the session-as-task loop

Ticket: mgschiebel/libharness#26. Branch `prototype/session-core` (based on `prototype/session-task`), never merged to main.

**Question:** can the sans-IO core from #6 (`prototype/sans-io-host`) be the engine inside the session-as-task async loop from #19, and what does the composition cost?

**Answer in one paragraph:** yes. The core's real code (432 lines, 11 changed lines against the original) drives the session. All nine session-task scenarios pass unchanged on tokio, futures-executor `ThreadPool` and `LocalPool` (27 tests, 0 failures in 80 repeated runs). The policy half of the session (typed result, bounded retries, nudge, turn limit, steer/follow-up ordering, abort, model failure, tool timeout) is testable with no executor and a fake clock (13 pure tests, 0.00s). The price: the library grows from 513 to 719 lines of our own code (1151 with the core), about 300 lines of it is translation between core and async loop, the core needed a 9-line patch to express "stop after the answer", two behaviours differ from session-task, and the abort-final-poll and cancel hooks stay in untestable-without-executor async code. Plain async loop is simpler to read. The sans-IO engine is simpler to test.

## Run

```
cd prototypes/session-core
cargo test --workspace                 # everything
cargo test -p session-core             # pure-logic tests only: no executor, no futures crate (13 + 1)
cargo test -p sans-io-core             # the core's own 10 scripted tests, unmodified apart from the crate name
cargo test -p host-tests-core          # the nine scenarios on session-core: 9 tokio + 9 ThreadPool + 9 LocalPool, + 4 extra
cargo test -p host-tests               # same on the baseline session-task (kept in this workspace for comparison)
cargo tree -p session-core             # serde, serde_json, sans-io-core (+ proc-macro crates)
```

## Layout

- `sans-io-core/src/lib.rs` the core, copied from `prototype/sans-io-host` (`src/lib.rs` minus the `pub mod toy;` line). Three patch hunks marked `PATCH`. `tests/scripted.rs` is the original 10-test file (only the crate name changed).
- `session-core/` the new library, same public API as session-task plus a public pure `Engine`:
  - `src/engine.rs` (249) the **pure half**. Owns an `sio::Agent`. Inputs carry `now`; `poll()` yields `Action::{CallModel, RunTool, Event, Finish}`. Does the type translation, intercepts `final_answer` (typed result, retries), nudges, counts turns. No async, no clock reads.
  - `src/session.rs` (195) handle + the **async driver** (`drive`, 97 lines): runs actions as registered callbacks, feeds results back. `abortable` and the handle are unchanged from session-task.
  - `src/lib.rs` (188, +2 lines vs session-task), `src/cancel.rs` (87, byte-identical).
- `session-task/` the baseline library, untouched copy (513 lines). `host-tests/` the baseline harness, untouched copy plus one new file `tests/extra.rs`.
- `host-tests-core/` has no source of its own. Its `Cargo.toml` points `[lib]` and `[[test]]` at the **same files** in `../host-tests` and renames the dependency (`session-task = { path = "../session-core", package = "session-core" }`). So `glue.rs`, `scenarios.rs`, `tokio_exec.rs`, `futures_executor.rs` are compiled byte-for-byte unchanged against the new library (`diff -r` against the session-task copy shows only `extra.rs` added).

## Assumptions made (AFK)

- Reuse the real core, patched only where it cannot express a session-task behaviour; every patch and every divergence is counted below.
- Keep session-task's public API and semantics as the target (`Capabilities`, `Session::start`, `SessionHandle`, `on_result`, `CancelToken`, `abortable` contract).
- `final_answer` stays a tool, but the engine resolves it in-process rather than handing it to a callback.
- The async driver reads `std::time::Instant::now()` for `now` (std only, not a runtime type). The pure layers never read a clock. A real design might make the clock a registered capability; not built.
- Tool timeouts: the engine accepts `tool_timeout` and the core's deadlines work (tested with a fake clock), but the async driver passes `None`, because waiting for a deadline needs a timer, which is a runtime. Out of scope, as in #19.
- No streaming: a model reply is fed to the core as `TextDelta` (if non-empty), one `ToolCall` per call, then `Done`.

## Validation points

| # | Point | Result | Evidence |
|---|---|---|---|
| 1 | Async loop only awaits registered callbacks and feeds results to the core as inputs; core outputs become callback calls | **YES** | `drive` (session.rs) holds one `Vec<BoxFut<Done>>` of in-flight callbacks. `Action::CallModel` -> `model(req)`, `Action::RunTool` -> the registered tool. Completions go back via `model_done` / `tool_done`. The only other awaits are the cancel token and the `on_result` callback. `final_answer` never reaches a callback (pure test `typed_result_after_a_tool`). |
| 2 | The nine scenarios pass unchanged on tokio and futures-executor | **YES** | `cargo test -p host-tests-core`: 9 tokio + 9 `ThreadPool` + 9 `LocalPool` pass; same files as the baseline (see Layout). 80 consecutive full runs, 0 failures. |
| 3 | Core's explicit `now` and scripted-input tests run with no executor | **YES** | `cargo test -p sans-io-core`: the original 10 tests pass on the patched core (0.00s). `cargo test -p session-core --test engine_pure`: 13 tests, plain `#[test]`, fake clock (`t0 + Duration`), no `async`, no `futures`, no threads. `tool_timeout_with_a_fake_clock` expires a tool deadline through the engine without sleeping. |
| 4 | No runtime types in the API, no I/O-capable dependency | **YES, same caveat as #19** | `cargo tree -p session-core`: serde, serde_derive, serde_json, itoa, memchr, zmij, syn/quote/proc-macro2/unicode-ident, sans-io-core (zero deps) = 15 entries vs 14 for session-task. `tests/no_runtime.rs` scans the sources and manifests for tokio/futures/smol/async-std/std::{fs,net,io,thread,process}/spawn/sleep (only allowed clock read: `Instant::now` in session.rs). 0 `unsafe`. "Cannot do I/O" is still by inspection (ticket #18). New public items: `Engine`, `Action`, both sync and runtime-free. |
| 5 | Steering, follow-up, abort, typed result, bounded malformed retry as in session-task | **YES, with 2 divergences and 1 patch** | Scenarios `steer_follow_up`, `abort_running`, `typed_result`, `malformed_results` pass. Divergences: multiple queued steers (one per boundary instead of all at once) and a steer sent before the future is first polled (arrives one turn later). Both measured by `host-tests/tests/extra.rs` on both engines, see below. Needed core patch for stop-after-answer. |

### Divergences measured on both engines (`tests/extra.rs`, `--nocapture`)

| Case | session-task | session-core |
|---|---|---|
| Two steers queued during model call 1 | one extra request: user texts `[go, s1, s2]` | two extra requests: `[go, s1]` then `[go, s1, s2]` (core is one-at-a-time, Pi semantics) |
| Steer before the future is first polled | request 1 already has `early` | request 1 is `[go]`, `early` arrives in request 2 (steers only land at `after_turn` in the core) |
| `max_turns` = 3, model always calls a tool | `Err(MaxTurns)` after 3 requests | same |
| Unknown tool + 3-call batch | error result "unknown tool nope", results in source order | same |

Both divergences could be removed (drain all steers in the core; or inject pre-start steers into the first prompt), at the cost of more core edits. Not done, to keep the core near-verbatim.

## Measurements

### Lines

| What | session-task | session-core |
|---|---|---|
| Library, own code (`wc -l`, blanks and comments included) | 513 (lib 186, session 240, cancel 87) | 719 (lib 188, engine 249, session 195, cancel 87) = **+206 (+40%)** |
| Plus the sans-IO core | n/a | +432 (423 original + 9 patch) = **1151 total, 2.2x** |
| Async turn loop (`drive` + `join_all`) | ~145 | `drive` + `next_done` + `Done`: 97 |
| Pure policy+glue that used to live in the async loop | in `drive` | `engine.rs` 249 |
| Pure-logic tests, no executor | 0 | 13 (engine) + 10 (core, existing) + 1 source scan |
| Executor-bound tests | 27 (+4 extra) | 27 (+4 extra), same files |

**Translation layer between core and async loop:** the driver (97) plus the type/shape translation in `engine.rs` (about 210 of 249; the other ~37 are session policy: `final_answer` handling and the nudge/end decision) = **about 300 lines**. #19 estimated 35 to 90 lines by analogy to the sans-io-host loops. That was low by 3x or more, for these reasons: `args` is a `String` in the core and a `Value` in the session (JSON round trip per call, and the whole history is re-converted into session `Message`s on every `CallModel`); the core's `ToolResult` message has no tool `name`, so it is rebuilt by scanning earlier assistant messages; model replies are exploded into `ModelEvent`s; events are remapped (`final_answer` hidden, `Injected` reconstructed by diffing `QueueUpdate` counts); and everything session-specific (typed result, nudge, turn limit, error mapping) has no home in the core.

### What logic moved from async code into pure tests

Moved into `engine.rs` and covered by plain `#[test]`s: typed result and malformed-answer bounded retry (3 tests), nudge then `NoAnswer`, steer at the boundary and follow-up at the stop, one-steer-per-boundary, abort mid-tool and mid-model (idempotent, stale inputs ignored), `max_turns`, model failure mapping, tool-result source ordering when completions arrive out of order, tool timeout with a fake clock, event mapping. Roughly the whole `drive` body except awaiting.

Stays in async code (needs an executor to test): in-flight polling and wake-ups (`next_done`, 18 lines), the "poll once more after cancel" contract (`abortable`, 17), the shared steer/follow queues and when they are drained (about 10 lines in `drive`), the cancel token (87, byte-identical, hook semantics), callback boxing. About 130 lines, down from about 250 (`drive`, `join_all`, `abortable`, `cancel.rs`) but not zero.

### Behaviour the core could not express

| Behaviour | Result |
|---|---|
| Stop after the accepted answer (terminal tool) | **Not expressible.** The core always runs another model turn after a tool batch. Without a patch, 6 of 9 scenarios fail (`typed_result`, `malformed_results`, `steer_follow_up`, `parallel_sessions`, `spawn_depth_limit`, `spawn_fanout_limit`: the extra model call hits an exhausted script). Fixed with `Agent::mark_done()`: 1 field, 1 method, 1 changed line (`continue_run = had_tools && !take(done_after_batch)`), 11 diff lines, the 9 net lines in the line table. |
| Typed result and malformed retry | Not a core concept. Fits entirely in the engine on top of the tool path (about 22 lines) plus the patch above. |
| Abort final poll and sync cancel hooks | **Not expressible.** The core's `CancelModel`/`CancelTool` are advisory outputs; session-task's contract (hooks run synchronously on `abort()`, in-flight futures polled once more before drop so an inline child can report `Aborted`) lives in the token and `abortable`. The engine ignores `CancelModel`/`CancelTool` entirely. `cancel_inline_child` and `cancel_reaches_spawned_child` pass because that code is unchanged. |
| Turn limit | Not in the core. The engine counts `TurnStart` and abandons the core mid-run. |
| Nudge | Works by calling `prompt()` on the idle core, which is a second "run" (`AgentStart` twice). Fine for history, odd for event consumers. |
| Tool timeouts | Core supports them; the loop cannot wait for a deadline without a timer capability, so unused in the async driver. |
| Steering semantics | Two divergences (table above). |
| Tool name in `ToolResult` | Core drops it; reconstructed. |

## Plain async loop vs sans-IO engine

**Simpler to read: the plain async loop.** `drive` in session-task is one function, one place, about 120 lines, and the whole turn is visible top to bottom. In session-core you follow four layers: the driver, the engine's `translate`/`event`, the core's `Phase` machine, then back out through `to_session`. The policy is split across the engine (`run_ended`, `final_answer`) and flags inside the core (`done_after_batch`, `pending`, queues), and one has to know that the core pops one steer per `after_turn`. Reading "what happens after a bad final answer" takes three files in session-core and one function in session-task.

**Simpler to test: the sans-IO engine.** A test is a script of inputs and a list of expected actions; no `launch`, `Spawner`, oneshot gates, `Step::held`, executor, or three-executor matrix. 13 tests run in 0.00s and were written without a single sleep or gate. They also found a real bug on the first run: a stale `model_done` after abort still emitted a `Tokens` event (fixed by an `if self.finished` guard). The tool timeout is only testable in the engine; the async loop cannot do it at all. Counterweight: session-task's async scenarios were already deterministic and fast (27 tests in about 0.01s, 0 flakes in 80 runs once the one early race was gated), so "untestable async code" overstates the baseline problem. The gain is "no executor needed, fake clock", not "testable at all".

## Surprises

1. **The core could not express "the answer ends the run"**, the most basic session-task semantic. The core's loop is "tools in, model again". Needed a patch; without it two thirds of the scenarios fail.
2. **Translation was about 4x the estimate** (about 300 lines vs 35 to 90). Mostly shape mismatches, not logic.
3. **Same test files, two libraries** works cleanly with a renamed path dependency (`package = "session-core"`), no feature flags and no cfg. Good trick to prove "unchanged".
4. **Behaviour drifted in places no scenario checked**: multi-steer and early-steer (measured above); `ToolEnd` events now arrive in completion order (core), not source order (session-task); a malformed `final_answer` mid-batch now drops the other in-flight tools instead of waiting for them; session-task's quirk where a stale earlier answer ends the session after a later non-answer tool batch is gone. Only `extra.rs` and the pure tests pin down a few of these.
5. **Everything passed first try on the nine scenarios** once the patch existed, which says more about the scenarios (they exercise the happy semantics) than about equivalence; the divergences above were found only by writing extra cases.
6. **A clock read slipped into the pure layer** (`Engine::new` called `Instant::now()`); caught while writing the source scan, now a constructor argument.
7. `Message::ToolResult.name` and `ToolCall.args: Value` are session-layer facts the core does not carry, so the "real code" reuse forced a lossy-looking mapping layer rather than a thin shim.

## Recommendation for #20 (non-binding)

- Composition **works** and keeps the runtime-neutral API, the nine scenarios and the 3-executor matrix unchanged. It is a viable design.
- It is not free: +206 lines of our own code, the core on top (2.2x total), about 300 lines of mapping, one core patch, two semantic divergences and still about 130 lines of async code that needs an executor to test (the cancel contract). The benefit is mostly pure-logic tests with a fake clock for the policy, and a place for timeouts to be tested.
- Plain loop wins on readability; engine wins on testability. If the loop stays this small (about 120 lines) the plain loop is the lower-risk choice and executor-bound scenario tests are adequate.
- If the sans-IO core is wanted anyway (host-facing sans-IO API from #6, timeouts, fake clock), the mismatch cost drops by designing the core for it from the start instead of retrofitting: generic or `serde_json::Value` args, tool `name` on result messages, a terminal-tool or "run done" concept, an option to deliver all queued steers at once, and a first-class abort that carries the "final poll" idea. Then the async layer shrinks to about the 100-line driver plus the cancel token.
- Either way keep `CancelToken` and the `abortable` final-poll rule in the async layer: no sans-IO core can own them.

Caveat: one toy with scripted fakes, no streaming, no real timers, no persistence; the line counts include blanks and comments.
