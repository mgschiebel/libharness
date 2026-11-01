# PROTOTYPE (throwaway): sans-IO host interface

Ticket: mgschiebel/libharness#6. Lives on branch `prototype/sans-io-host`, never merged to main.

**Question:** does a sans-IO agent-loop state machine feel right as the host-facing interface, and how verbose is the host side?

## Run it

```
cd prototypes/sans-io-host
cargo test                          # 10 deterministic tests, scripted fake model + fake clock, no threads/sleeps
cargo run --example host_sync       # std threads + mpsc host
cargo run --example host_async      # tokio host (same scenarios, same output modulo timings)
```

Scenarios in both hosts: (A) prompt, two parallel tool calls, a steer and a follow-up arriving mid-tools; (B) abort during a slow tool; (C) tool timeout.

## Layout

- `src/lib.rs` (about 420 lines incl. types): the core. No I/O, no async, never reads a clock.
- `src/toy.rs`: fake model, fake tools, scenarios, and `feed()` (HostMsg -> core call). Shared by both hosts.
- `examples/host_sync.rs` (66 lines), `examples/host_async.rs` (86 lines): the host loops.
- `tests/scripted.rs`: the deterministic tests.

## The interface

Inputs (each takes `now: Instant`; the core never calls `Instant::now()`, str0m style):

```rust
agent.prompt(now, text) -> Result<(), Busy>       // Busy if a run is active (Pi session layer: must pick steer/follow-up)
agent.steer(now, text)                             // delivered after the current turn's whole tool batch
agent.follow_up(now, text)                         // delivered only when the agent would otherwise stop
agent.handle_model_event(now, call_id, ModelEvent) // TextDelta | ToolCall | Done(StopReason) | Failed
agent.handle_tool_result(now, tool_call_id, ToolResult)
agent.handle_timeout(now)                          // spurious calls are no-ops (quinn-proto contract)
agent.abort(now)
```

Outputs, drained with `poll_output()` until `None` after every input:

```rust
enum Output {
    CallModel(ModelRequest { call_id, messages }),
    CancelModel { call_id },
    RunTool(ToolCall),               // all calls of a batch are emitted together => host decides concurrency
    CancelTool { tool_call_id },
    Event(AgentEvent),               // Pi vocabulary
}
```

Plus `poll_timeout() -> Option<Instant>` (earliest tool deadline), `is_idle()`, `messages()`, `take_queued()`.

`AgentEvent` mirrors Pi's (research/pi-sdk.md section 3): `AgentStart/End{aborted}`, `TurnStart/End`, `MessageStart/Update/End`, `ToolExecutionStart/End{duration}`, plus `QueueUpdate{steering,follow_up}` from Pi's session layer. Omitted: `tool_execution_update`, `agent_settled`, retry/compaction events.

Semantics copied from Pi's `runLoop` (pi-sdk.md section 2 and 5): steering waits for the whole tool batch, tools are never skipped; follow-ups only when no tool calls and no steering; `length` stop fails tool calls instead of running them; `error`/`aborted` ends the run; tool-result messages in source order while `ToolExecutionEnd` events are in completion order. Queues are one-at-a-time.

Per-call correlation ids (`call_id`, `tool_call_id`) make stale inputs after abort harmless: the core ignores them.

## Host-side verbosity (measured)

The whole host loop, minus scenario/user-thread plumbing, is about 35 lines in both hosts:

1. `while let Some(out) = agent.poll_output()`: print events, spawn model task, spawn tool tasks, cancel.
2. Wait on one channel with a deadline from `poll_timeout()`.
3. `feed()` the message (or `handle_timeout` on expiry), loop.

Sync (std threads, `recv_timeout`) vs async (tokio, `select!` + `sleep_until`): the loops are structurally identical. Async is longer only because of `AbortHandle` bookkeeping (about 20 lines) and the `select!` deadline arm. This supports the research claim (sans-io-rust.md section 3) that one loop shape serves sync and async. Output of both hosts was identical apart from timing jitter.

What the host must still write that an `async fn run()` would hide: the model-streaming task (turn SSE chunks into `ModelEvent`s), the tool-dispatch task, the channel, the deadline wait, and cancellation bookkeeping. About 35 lines for a toy; a real one adds HTTP/SSE parsing, which it needs anyway. I estimate the loop itself stays at roughly 50 lines in real use, but that is a guess: this prototype has no real HTTP, no persistence and no permission prompts.

Time as an input paid off in tests: the timeout test advances a fake clock and never sleeps. All 10 tests run in 0.00s and are fully deterministic, because the scripted model is just a `Vec<ModelEvent>` fed by the test.

## What felt awkward

1. **Every host reimplements the same loop.** The `poll_output` match plus the select/deadline wait is boilerplate that is the same for every embedder. This is the known sans-IO cost (sans-io-rust.md section 4 item 8) and it is real here.
2. **Drain rule is convention, not enforced.** Forgetting to drain after `feed` silently stalls the agent (no `CallModel` ever seen). Nothing in the types prevents it. A driver, or making inputs return an iterator/`Drain<'_>` borrowing the agent, would fix it.
3. **Correlation ids everywhere.** The host must thread `call_id` and `tool_call_id` through spawned tasks. Easy to get wrong (sending a result with the wrong id is silently ignored).
4. **Cancel is advisory.** `CancelModel`/`CancelTool` are requests; the sync host cannot cancel threads at all and relies on the core ignoring stale results. Fine, but it means the core must tolerate late input forever (tested).
5. **Abort leaves event gaps.** `abort()` synthesizes error tool results to keep history valid, but emits no `ToolExecutionEnd` for them. Pi's contract on this was not checked; a UI would want those events.
6. **Steering granularity.** With parallel tools, "interrupt after the current tool" really means after the whole batch (Pi semantics, kept here). A host wanting to interrupt a long tool must `abort()` or cancel the tool itself and feed back an error result; the core offers no "skip remaining tools" input.
7. **Idle `steer`/`follow_up` start a run.** Convenient but surprising; Pi's session layer rejects an unqualified prompt while streaming but accepts steer/follow-up queues. Needs a decision.
8. **Streaming tool-call args are out of scope.** `ModelEvent::ToolCall` carries a complete call. openrouter-api.md says streaming tool-call deltas are unverified, so partial args would need a new input variant, and the host (or an adapter crate) accumulates fragments for now.
9. **No hooks.** Pi's `beforeToolCall`, `afterToolCall`, `transformContext`, `prepareRequest` have no place here. In this shape they are just host code between `poll_output` and `handle_*` (block a tool = feed an error result instead of running it; transform context = edit `ModelRequest.messages` before sending). That is arguably nicer than callbacks (no `Arc<Mutex>`, no async closures), but messages are cloned into every `CallModel` (cheap here, costly for long histories) and the host cannot mutate core history except via results.

## Open questions

- Should `Output` carry borrowed data (`&[Message]`) instead of clones? Needs a lending-iterator-ish API or an index handle.
- Who owns session persistence: host reading `Event::MessageEnd`, or the core exposing `messages()` diffs? (Next ticket.)
- Do we need a `Settled` event like Pi's `agent_settled` once retry/compaction exist? Here `AgentEnd` is final.
- Timeouts: only tool deadlines modelled. Model-call idle timeout, retry backoff and rate-limit waits would all become more deadlines; probably a generic `poll_timeout` over all of them.
- Is `serde` on all types and snapshot/restore of the machine wanted? State is plain data so it looks cheap, but untested here.
- Should `RunTool` carry the tool's execution mode (Pi `sequential`/`parallel`) or leave concurrency fully to the host? Currently fully to the host.

## Recommendation: sans-IO core plus an ergonomic driver

Not "pure sans-IO core only", and not "hook-driven loop instead".

Evidence:

- The core worked cleanly. Every behaviour in the ticket (prompt, streaming, parallel tools, steering, follow-up, abort, timeout) fit as plain inputs/outputs with no `Arc<Mutex>`, no callbacks, no runtime. 10 deterministic tests with a scripted model and fake clock ran instantly with no mocks or I/O. That is a large testability win over a hook-driven async loop, where the same tests need a runtime and mock futures.
- The host loop was small (about 35 lines) and identical for std threads and tokio. So the core does not lock us into a runtime, which settles the "async runtime" open item on the map in favour of not choosing one in the core.
- But the loop is boilerplate every embedder repeats, and the main footguns (undrained outputs, id threading, cancel bookkeeping) are host-side mistakes the types do not prevent. The research (sans-io-rust.md section 3 and 5) points to the same layering quinn uses: `quinn-proto` plus `quinn`. The sans-IO core is the contract and test surface; a thin driver (optional, behind a feature, tokio first) takes a `Provider`-like model trait and tool trait and gives `async fn run()` plus an event stream.
- Against hook-driven: Pi's hooks exist because its loop owns I/O. With a sans-IO core, each Pi hook collapses into ordinary host code at a visible point (see awkward item 9), so hooks can live in the driver as sugar rather than in the core.

Caveats on this evidence: it is one toy with a fake model and instant tools; no real SSE, no persistence, no large histories (clone cost), and no real cancellation semantics for HTTP. The 35-line host figure will grow. The recommendation is a design direction to confirm in the grilling step, not a measured result.

Suggested follow-ups for the spec: enforce the drain rule in types, define the `ToolExecutionEnd`-on-abort contract, decide idle steer/follow-up behaviour, and decide the borrowed-vs-cloned request question.
