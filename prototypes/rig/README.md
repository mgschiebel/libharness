# Prototype: rig 0.46 against libharness requirements

PROTOTYPE, throwaway. Answers issue #10: where does rig satisfy libharness (embedded, host-driven, host owns side effects, provider by URL starting with OpenRouter) and where does it force compromises?

Run: `cargo run` in this directory (needs rustc >= 1.97.1). A local axum mock stands in for OpenRouter at `http://127.0.0.1:<port>/api/v1`; no network. Code: `src/main.rs`. Crates: `rig-core` (feature `reqwest`), `rig-agent`, `rig-memory` (feature `fs`), all 0.46.0.

## What was exercised and what happened

| # | Requirement | Result | Evidence |
|---|---|---|---|
| 1 | Prompt + custom OpenAI-compatible base URL | Works | `OpenAIConfig::with_key(&OPENROUTER, key).with_base_url(url).client().chat("openai/gpt-x")`. Mock saw `Authorization: Bearer ...`, model id, `tools` array, on `/api/v1/chat/completions`. |
| 2 | Tool call, high-level agent | Works, one gotcha | Typed `Tool` trait; model-issued call ran, result went back, final text returned. Default `max_turns` is 1, so a tool round trip fails with "reached the max turns limit of 1" until `.max_turns(n)` is set per prompt. |
| 3 | Session persistence | Works | `AgentBuilder::memory(DirStore).conversation("s1")`. A second agent value on the same log sent 5 messages (4 kept + new prompt). JSON-lines log in `logs/`. Store trait is pluggable (`AsyncStore`). Failed or cancelled runs save nothing; save is append-only with a compare-and-append length check. |
| 4 | Compaction | Works, sans-IO | `CompactionPolicy::plan_summary` returns a `CompletionRequest`; the host sends it, `Summarizer::summary_text` parses it, `apply_summary` splices it in (6 msgs -> 2). `ClearToolOutputs` frees context with no model call. Defaults are explicitly modelled on pi. Needs a `ModelSpec` (context window); `ProviderId` is awkward to construct (`ProviderId::catalog("openrouter")` returns `Option`). Threshold/overflow triggers are the host's job here; the high-level agent did not auto-compact in my run (I did not enable it; untested whether `AgentBuilder` has an auto mode). |
| 5 | Steering / follow-up | Missing; emulable | `rig_agent::run::AgentRun` is a serializable sans-IO state machine (`next_step` -> `CallModel` / `CallTools` / `Done`). It has no queue and no way to inject a message mid-run (grep for steer/follow-up in rig-agent src finds nothing relevant). Emulation that worked: at a `CallModel` boundary after tool results, discard the run and start `AgentRun::new(steer_msg).with_history(run.full_history())`. Wire roles were `user, assistant, tool, user`. Costs: turn counter, `max_turns` budget, usage and `completion_calls` reset per run; the host must stitch them. Pi's guarantees (steer after current turn's tools, follow-up only when idle, one-at-a-time vs all modes) would all be host code. |
| 6 | OpenRouter extras | Works | `AgentBuilder::additional_params(json!({provider, models, reasoning}))` reached the wire body verbatim. Response-side extras (`cost`, SSE comments, mid-stream errors in HTTP 200) not tested. |

## Requirement fit

- **Embedded library:** yes. Plain crates, no binary needed. Native and wasm32-unknown-unknown supported; WASI not.
- **Host drives:** yes, via `AgentRun`. The README states the state machine "performs no IO", is `Serialize + Deserialize`, and builds without an async runtime when `rig-agent` has default features off (probe: `rig-agent --no-default-features` resolves 133 crates, none of tokio/reqwest/hyper; the full prototype tree is 228 crates).
- **Host owns side effects:** partly. With the stepper the host does model calls, tool execution and persistence itself (prototype step 5 did exactly that). But the idiomatic high-level `Agent` does model I/O, tool execution and memory saves internally; permissions only via hooks (`on_dispatch`/`on_outcome`). Using the stepper means giving up most of rig's convenience layer (hooks, memory integration, retries), and `prepare_request` / `RunSpec` / `ProviderCapabilities` wiring is on the host.
- **Provider by URL:** yes for OpenAI-shaped (OpenRouter has a built-in dialect with quirks). Transport is pluggable (`connect(DynHttpClient)`), so even HTTP can be host-owned.

## What forces compromises

1. **Steering/follow-up:** absent (above). Biggest functional gap versus Pi.
2. **Scope and churn:** 20+ providers, RAG, vector stores, ECS/Bevy harness, ACP; "Releases contain breaking changes". The sans-IO core is a small part of a large moving surface. Pre-1.0 (0.46).
3. **Type surface:** `ToolCall`/`Message`/`CallId`/`ToolName` newtypes, `Arc<Message>` histories, `ModelRef`, `Aside`/`Conversation` entry model. Rich, but libharness would inherit rig's message model rather than design its own. Session format is rig's (`rig-session/4`), not Pi's tree; rig has fork support but no verified tree-with-branches equivalent.
4. **Gotchas:** `max_turns` default of 1; errors from `AgentRun` protocol misuse are runtime errors; docs are mostly in source comments.

## Compile / API friction

- **MSRV:** all rig 0.46 crates require rustc 1.97.1; the sandbox had 1.97.0 and `cargo run` refused. `rustup update stable` (to cargo 1.99.1) fixed it. A libharness MSRV of "latest stable" would be forced if built on rig.
- `rig-core` needs feature `reqwest` for `.client()`; `rig-memory` needs `fs` for `DirStore`. Not default.
- `rig::`-facade vs `rig-core` naming split (`rig` on crates.io is now the coding agent/facade).
- Several constructors need hunting: `ToolDefinition::new` wants `ToolName` (fallible), `ProviderId::catalog` returns `Option`, `TurnPolicy::new(BTreeSet, ..)`, `ToolAnswer` is only obtainable via `ExecCall::answer`. After the first compile, every error was a 1-line fix.
- Not tested: streaming (SSE, tool-call deltas), mid-stream errors, `rig-rmcp`, hooks beyond reading signatures, auto-compaction wiring in `AgentBuilder`.

## Recommendation: build from scratch, borrowing rig's ideas (do not adopt); optionally contribute steering upstream

Evidence: rig 0.46 already satisfies most of the checklist, which is the surprise, but the fit is wrong in two ways.

1. libharness's stated shape is a Pi-minimal, sans-IO, host-owns-everything core. Rig's sans-IO piece (`AgentRun`) exists but is the low-level escape hatch of a much larger framework; the ergonomic path is I/O-owning. Adopting means either living in the escape hatch (re-implementing hooks, retries, memory wiring, steering around it) or accepting the framework's ownership of side effects, contradicting the map's leaning.
2. The missing piece (steering/follow-up queues, the Pi core feature) can only be emulated by restarting runs with split budgets and usage.

What would flip this to "build on rig": the user prefers shipping fast over Pi-fidelity and accepts rig's message model and churn, or upstream accepts a queue-aware `AgentRun` (a contribution of modest size given the state machine is already pure). Cheap, reversible middle path: build libharness core from scratch with its own small types, and keep `rig-memory` compaction/summary prompts as a reference (they copy pi's), and consider a thin adapter that lets a libharness host call rig-core providers as a model backend (`rig-core` providers are separable from `rig-agent`).
