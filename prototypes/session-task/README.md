# PROTOTYPE (throwaway): session-as-task with injected async capabilities

Ticket: mgschiebel/libharness#19. Branch `prototype/session-task`, never merged to main.

**Question:** does the session-as-task model feel right and stay runtime-neutral in Rust? The host registers capabilities (model, tools, result sink) as async callbacks, starts a session with a prompt, and the session talks to the outside only through them.

## Run

```
cd prototypes/session-task
cargo test                      # 27 tests: 9 scenarios x (tokio multi-thread | futures-executor ThreadPool | futures-executor LocalPool)
cargo tree -p session-task      # serde + serde_json only
```

Ran the whole suite 80 times in a loop with 0 failures (one early flake was a test race, fixed; see Surprises).

## Layout

- `session-task/` the library. `lib.rs` (types + capability registration), `session.rs` (handle + turn loop), `cancel.rs` (cancel token). Deps: `serde`, `serde_json`. No `futures` crate at all, only `std::future`.
- `host-tests/src/glue.rs` scripted fake model, fake tools, `launch()`. `scenarios.rs` the nine scenarios as `async fn(Spawner)`. `tests/tokio_exec.rs` and `tests/futures_executor.rs` are 16 and 38 lines and only differ in how they build a `Spawner`. The scenario code is byte-identical on every executor.
- Reuse of `prototype/sans-io-host`: **semantics only, not code** (steer at turn boundary, follow-up only when stopping, tools in a batch concurrent, queues). Its core is a synchronous state machine driven by a host loop with explicit `now`; the ticket's model has the library own the loop and await callbacks, so I wrote a small async turn loop (about 150 lines) rather than wrapping the core. See Recommendation for how the two can still combine.

## Shape of the API

```rust
let caps = Capabilities::<Verdict>::new(verdict_schema())           // R chosen here by the host
    .model(|req: ModelRequest| async move { Ok(ModelReply{..}) })    // the model call is a capability
    .tool_typed("add", "add", schema, |ctx: ToolCtx, i: AddIn| async move { Ok(i.a + i.b) })
    .tool("echo", "echo", schema, |ctx, json| async move { Ok(json) })
    .on_result(|id, r: Result<Verdict, SessionError>| async move { /* single terminal callback */ })
    .on_event(|e| { /* sync, observability */ });                    // Capabilities is Clone, reusable across sessions

let (handle, fut) = Session::start(caps, SessionConfig::new(id), "prompt");  // fut: impl Future<Output=SessionEnd> + Send + 'static
spawn(fut);                                   // the HOST runs it; the library never spawns
handle.steer("..."); handle.follow_up("..."); handle.abort();   // Clone + Send + Sync, no runtime types
```

Callbacks are plain `Fn(Ctx, In) -> impl Future + Send + 'static`; the library boxes them once at registration (`Arc<dyn Fn(..) -> Pin<Box<dyn Future + Send>>>`).

## Validation points

| # | Point | Result | Evidence |
|---|---|---|---|
| 1 | Same session code unchanged on two executors, no runtime types in API | **YES** | All 9 scenarios pass on tokio (4-thread), futures-executor `ThreadPool` and single-threaded `LocalPool`. `grep -n "tokio\|smol\|futures" session-task/src` finds one comment. Public API uses `std::future::Future`, `Pin<Box<..>>`, `Arc`, `Mutex`. |
| 2 | Handle: end/abort, steer, follow-up | **YES** | `steer_follow_up`: steer sent while model call 1 is in flight arrives in request 2 (turn boundary), follow-up only in request 3 after an answer was accepted; input after the end returns `false`. `abort_running`: abort during a hung tool gives `Err(Aborted)` and `SessionEnd::Aborted`. "End" is abort; there is no graceful end-now-with-partial-result (see Assumptions). |
| 3 | Two sessions in parallel, no shared state | **YES** | `parallel_sessions`: B (started second) finishes while A is still held; results are not mixed. The library has no statics; ids are host-supplied (a library-global id counter would have been shared state). |
| 4 | Child via host-provided spawn capability, library never spawns | **YES** | `spawn_depth_limit`, `spawn_fanout_limit`: `spawn_agent` is an ordinary `tool_typed::<SpawnIn, SpawnOut>`; the host closure builds child `Capabilities`, calls `Session::start`, and either awaits the child inline or hands its future to the executor. Library has no parent/child notion (grep: none). |
| 5 | Library crate has no I/O-capable dependency | **YES, with caveat** | `cargo tree -p session-task`: serde, serde_derive, syn/quote/proc-macro2, serde_json, itoa, memchr, zmij (14 crates). None opens files/sockets/threads, but "cannot do I/O" is by inspection and trust, not enforced; `serde_json` still has `std::io` entry points (`from_reader`) and `std` is available. Technical enforcement is ticket #18's job. |
| 6 | Typed result, host picks the type at registration, malformed answers | **YES** | `Capabilities::<R>::new(schema)`; the schema goes to the model as the `final_answer` tool (`{"result": <schema>}`), the library `serde_json::from_value::<R>`s it. Malformed: the error and expected schema go back to the model as an error tool result; after `max_result_retries` (2) the host gets `Err(Malformed{attempts, last_error, raw})`. Model never calling it: one nudge, then `Err(NoAnswer)`. `R = String` works. Test `malformed_results`. |
| 7 | Cancellation reaches a child | **YES** | Every callback gets `ctx.cancel: CancelToken` (see below). `cancel_reaches_spawned_child`: aborting root while the grandchild-level tool hangs -> the spawn callback's `ctx.cancel.on_cancel(|| child.abort())` fires and the host-spawned child reports `Err(Aborted)` through its own `on_result`. |
| 8 | Model call is a registered callback, scripted fakes | **YES** | `.model(..)`; `scripted()` and the reactive `tree_model` in `host-tests`. No network anywhere. |
| 9 | Host-side depth limit | **YES** | Depth is a closure-captured integer in `tree_caps(host, depth, ..)`; at `depth >= max_depth` the callback returns `Err("depth limit reached")`, which the model sees as a tool error (3 sessions created, max depth 2). Fan-out is a per-session counter in the same closure (3 parallel spawns, limit 2: two run, one refused). |

### What the library hands a callback for cancellation

`CancelToken` (std only: `AtomicBool` + `Mutex<{wakers, hooks}>`, 87 lines):

- `is_cancelled()`
- `cancelled() -> impl Future<Output=()>` for `select`-style code (any executor's select works)
- `on_cancel(FnOnce + Send)` synchronous hook, run immediately on `abort()` on the aborting thread (or at once if already cancelled). This is what stops the child: `ctx.cancel.on_cancel(move || child_handle.abort())`.
- `child()` for derived tokens.

Policy on abort: set the flag, run hooks, wake, poll the in-flight callback future once more, then drop it. There is no grace period: a grace timeout needs a timer, which is a runtime. A callback future is dropped at abort, so cleanup in async code after `cancelled().await` never runs; only the sync hook and `Drop` are reliable. The one extra poll is why an *inline* child (child future awaited inside the parent's callback) still reports `Aborted` (`cancel_inline_child`); it is a library contract I added deliberately (see `abortable`), and without it that child would vanish with no terminal callback.

## Measurements

| What | Lines |
|---|---|
| Library total (`wc -l`, including blanks and ~37 comment lines) | 513 = lib.rs 186, session.rs 240, cancel.rs 87 |
| Of which turn loop (`drive`) | ~120 |
| Host glue, generic (scripted model, tools, `launch`, `Spawner`) | 127 (`glue.rs`) |
| Host glue for the spawn capability + depth/fan-out/cancel wiring | ~130 (`scenarios.rs`, section "spawn as an ordinary callback"); the spawn callback itself is ~35 |
| Per-executor test harness | tokio 16, futures-executor 38 (of which ~10 is the LocalPool forwarder) |
| Library dependencies | 2 direct (serde, serde_json), 14 total, 0 `unsafe`, 0 `futures` |

**Callback signature cost.**
- Boxing: one `Box<dyn Future>` allocation per callback invocation, hidden inside the library (`BoxFut<T> = Pin<Box<dyn Future<Output=T> + Send + 'static>>`). Hosts write ordinary `async` closures; no `BoxFuture` or `Box::pin` appears in host code for registration.
- `'static` + `Send` + `Sync` on every callback and future. This is the real cost: capabilities clone `Arc`s into `async move` blocks (e.g. `let host = host.clone(); async move {..}` in every closure, seen in all the glue), and the futures cannot borrow from the host's stack.
- `Send` leaks into the host: a `!Send` spawner (futures `LocalSpawner`) cannot be captured by a callback, so the LocalPool run needed a channel-forwarder task (about 10 lines). A single-threaded host that wants non-`Send` state would need a second, `?Send` flavour of every signature, which means either duplicating the API or making it generic over a marker trait.
- Lifetimes: none surfaced in the public API (everything is owned/`'static`). Inputs are owned `Value`/`I`; tool inputs are deserialized for the host (`tool_typed`).
- `on_event` is sync, to avoid a boxing and a back-pressure question; it must not block.

**Anything that forced a runtime choice into the library:** nothing. Pressure points that I resolved without a runtime: (a) waiting for control input: not needed, since a session never idles awaiting input (steer/follow-up are queues polled at turn boundaries; the future ends when it would stop); (b) concurrent tool batch: a 20-line hand-rolled `join_all` instead of `futures`; (c) abort racing an await: a hand-rolled `poll_fn` select; (d) grace-period timeout on abort: **not possible** without a timer, so dropped (host can add a deadline on its side); (e) tool timeouts: same, host-side only. Timers are the one thing the model cannot express without a host capability (e.g. a registered `sleep` callback, not built).

## Assumptions I made (AFK)

- The library owns the turn loop; `Session::start` returns `(handle, future)` and the host decides where the future runs. "The host does not wait" holds: results arrive via `on_result`; `SessionEnd` from the future is a convenience.
- One terminal callback, `on_result(id, Result<R, SessionError>)`, called once per session whose future completes, including abort and failure. If the host drops the future, nothing is called.
- The result is delivered through a built-in `final_answer` tool whose parameter is the host's schema (hosts hand the schema as JSON; no `schemars` dependency). The library cannot derive a schema from `R`; the host supplies one (a derive could be an optional adapter).
- Abort is the only "end". No "finish now, return what you have".
- Ids are host-supplied.
- Steering is delivered at turn boundaries only, never while a model call is in flight. Last accepted `final_answer` wins when follow-ups continue the session.
- Streaming is not modelled: a model reply is one value.
- Tool errors are strings shown to the model; no retry policy, no per-tool timeouts.
- Cost/trace roll-up not built, only reasoned (below).

## Surprises

1. **Abort needs a "final poll" contract.** Plain `select`-then-drop kills an inline child before it can report. Hooks alone were not enough until `abortable` polled the future once more after cancel. Whether to promise this in the API is an open choice.
2. **Spawning needs no library concept, but cancellation did need a library primitive** (hook-capable token). `Drop`-only cancellation works for inline children but not for host-spawned ones (the child lives on the executor, so dropping the parent callback does not touch it).
3. **`Send` bounds reached the host** (LocalPool needed a forwarder).
4. **The follow-up path had a bug in my first cut** (follow-up was only injected when the model replied without tool calls). Real semantic: follow-up must also fire when a valid answer is accepted and the session would stop.
5. **Concurrency in tests is racy by default:** my first fan-out test flaked (a fast child finished before the parent's third call checked the limiter). Fixed with a gate. Host-side limits need to be written with that in mind: a counter checked at call time only sees children that are still alive.
6. Zero `futures` dependency was easy; the library needed only `std::future::poll_fn` and `pin!`.

## Spawn: what forces a parent/child concept (note on #21)

- Nothing was forced. The child is an independent session behind a callback.
- **Tracing ids:** the callback gets `ctx.session`; the host puts the parent id in the child's `SessionId`/config. Events carry the session id, so correlation is host-side. A `parent` field in `SessionConfig` would be trivial but is not needed.
- **Cost roll-up:** `ModelReply.tokens` surfaces as `Event::Tokens`; the host sums per tree in its own `on_event`, keyed by ids it assigned. The spawn callback can add the child's total to the parent's tally since it sees both. No library support needed, but the library does not do it for you.
- **Cancellation:** needed the `CancelToken` hook (above). That is the only library-level support spawning wanted, and it is useful for any long-running tool.
- **Inheritance:** nothing inherited; `tree_caps` rebuilds each child's capabilities (shows both a fan-out and a depth counter closed over by the host).
- **Steering a child:** the host holds the child's `SessionHandle` (the glue keeps them in `Host.handles`); nothing forwards steering from parent to child automatically.

## Recommendation (feeds #20; not a decision)

The model holds up. Registration, handle, typed result, spawn-as-callback and cancel all worked with a small, dependency-free library (about 500 lines) that ran unchanged on three executor configurations. Main variants to weigh:

1. **Boxing/Send flavour.** Current prototype: boxed `Send + 'static` futures. Alternatives: generic callbacks via traits with associated future types (no allocation, much heavier signatures and no `Arc<dyn>` registry), or a `?Send` mode for single-threaded hosts. I would start with the boxed `Send` form; the cost is clones in host closures, not performance.
2. **Cancellation contract.** Keep the token-with-sync-hooks plus the "one more poll" guarantee, or reduce to `Drop` only, which breaks host-spawned children. The token is cheap (87 lines, std only).
3. **Timers.** Anything with time (tool timeouts, abort grace, retries with back-off) has to become a host capability or host-side wrapper. Decide that explicitly rather than let it creep in.
4. **Session engine.** Own async loop (as here) versus embedding the sans-IO core from #6 inside the async loop: the core would receive `ModelEvent`/`ToolResult` inputs from the awaited callbacks and its `Output`s would be turned into callback calls. That keeps pure-logic testability and the fake clock; this prototype did not test that composition, and it would cost an extra translation layer (about the size of the sans-io host loops, 35-90 lines).
5. **Typed result source.** Host-supplied JSON schema (this prototype) versus a `schemars`-style derive adapter. Malformed handling (feed back, bounded retries, then typed error) was cheap; the retry count is a library knob.
6. **Streaming** is untouched: a model callback returning one reply does not show partial output. Options: callback gets an event sink (sync `Fn(Delta)`, as `on_event` here) or returns a `Stream`-like type (needs `futures-core`, still no runtime).
7. **Enforcement of no-I/O** is separate (#18): this prototype's dependency set is clean by inspection only.
