# libharness handoff

Written 2026-11-01. A Rust library embedding an LLM agent harness. Planning is driven by the `/wayfinder` skill (Matt Pocock's skills, installed in `.claude/skills/`). Repo: https://github.com/mgschiebel/libharness. No Rust crate exists on `main` yet; only research docs and planning.

## Where the plan lives

- **Map:** GitHub issue #1, "Map: libharness first public release spec". It holds the Destination, Notes (standing rules and leanings), Decisions so far, Not yet specified, and Out of scope. Read it first.
- Tickets are child issues of the map, linked with native sub-issues and native blocking.
- `gh` GraphQL is blocked in these sessions. Use REST: `gh api repos/mgschiebel/libharness/...`.

## Destination

A written spec for the first public release of libharness, ready to hand off for implementation.

## User's standing rules (also in the map Notes)

1. Decisions are data driven. Research tickets must link primary sources.
2. Research feeds prototype tickets, which test the real choices and produce a recommendation.
3. Grilling is only for identifying fog and for deciding from the research and prototype recommendations.
4. Smart blocking: a grilling ticket is blocked by the research and prototype tickets on its own topic.
5. Ask questions inline in chat as plain text. Never use menu or multiple-choice tools (voice-to-text user). This is also in `CLAUDE.md`.
6. A research ticket closes only after its findings are rebased onto `main` under `research/` AND its `research/<name>` branch is deleted. The sandbox proxy cannot delete remote branches, so the human does it.
7. Handoff and planning documents are plain Markdown files, not docs.

## Ticket status

| # | Ticket | State |
|---|--------|-------|
| 2 | Research: Pi SDK and agent-loop architecture | closed, `research/pi-sdk.md` |
| 3 | Research: OpenRouter API surface | closed, `research/openrouter-api.md` |
| 4 | Research: sans-IO design in Rust | closed, `research/sans-io-rust.md` |
| 5 | Research: Rust LLM crate landscape | closed, `research/rust-llm-landscape.md` |
| 6 | Prototype: sans-IO host interface | closed; branch `prototype/sans-io-host` |
| 7 | Prototype: provider abstraction over OpenRouter | open, unblocked, not started |
| 10 | Prototype: rig against libharness requirements | open; built on branch `prototype/rig`; awaiting the user's reaction |
| 11 | Grill: build vs adopt | open, blocked by #10; also blocks #8 and #9 |
| 8 | Grill: host-interface shape | open, blocked by #11 and #6's topic work |
| 9 | Grill: provider abstraction | open, blocked by #11 and #7 |

Findings in brief:
- **Pi:** two layers. Core is nested loops with hooks and steering/follow-up queues. The coding-agent layer adds append-only tree sessions, compaction as appended summaries, and about 40 extension events.
- **OpenRouter:** OpenAI-shaped Chat Completions plus extras. SSE has comment lines to skip, and errors can arrive mid-stream inside an HTTP 200. Streaming tool-call delta format is unverified.
- **Sans-IO:** synchronous state machine, explicit `now`, `poll_*` outputs. No primary source for sans-IO LLM agents; that mapping is inference.
- **Crate landscape:** provider clients are well covered (genai, async-openai, rig-core). Loops only ship bundled with their own provider layer. rig and swiftide overlap most.
- **Sans-IO prototype:** ~420-line core, 10 passing tests, host loops of 35 (core), 66 (sync) and 86 (tokio) lines. Recommended a sans-IO core plus a thin optional driver.
- **Rig prototype:** recommends building from scratch. rig's `AgentRun` has no way to inject a message mid-run, so steering is missing; the normal `Agent` does side effects itself, which conflicts with "host owns side effects". The user has not yet confirmed.

## Current design thinking (provisional, from the latest discussion)

The user described a sandboxed library model, not yet reflected in the map:

- The library does **no I/O of its own**. It reaches the outside world only through host-registered callbacks. Tools, skills, plugins, options and **the model call itself** are all registered capabilities. This also keeps credentials away from the model side and allows different models per session.
- The host registers capabilities up front. It then starts a **session** with a prompt. A session is an independent task (thread) that the host spawns and then carries on; the host hears back only when the session calls a host callback. The host does not wait on it.
- A session owns its own turn loop and runs to a natural end, or until the host ends it. A new prompt is a new session. Sessions can run in parallel.
- Sub-agents are child sessions. The library needs a way to spawn them.
- The host keeps live control: end/abort, plus steering and follow-up messages through a session handle.
- Callbacks are async Rust futures, and the library should be runtime-neutral. The user wants this prototyped.
- The user wants **technical enforcement** of "no I/O" from the start (not just convention). WASM is a possible later step.
- This is not a hook-driven loop. It is closer to actors or spawned tasks with injected capabilities. The sans-IO state machine can still be the session engine inside, with the session runner as the "ergonomic driver".

Unanswered questions I put to the user at the end of the session:
- **Continuity:** a session keeps its conversation state while running; the host stores it outside (via a persist callback or the final transcript); a later session is seeded from what the host stored. Is that right? (The user said this was fog and wanted to discuss.)
- **Spawning:** should spawning be a host-provided capability, so the library stays executor-neutral? Recommended yes.
- Go-ahead to reshape the map as below.

## Next steps

1. Get the user's answers on continuity and spawning, and their reaction to the rig prototype (#10). Recommendation there is to build from scratch.
2. Reshape the map: rewrite Notes and provisional leanings to the session-as-task model; reconsider the Pi-style "no sub-agents in core" leaning, which the session model reverses. Rename or reshape "Grill: host-interface shape" (#8) around it.
3. Add tickets:
   - Prototype: session-as-task with injected async capabilities, run on two executors to prove runtime neutrality.
   - Research: technical enforcement of "no I/O" in a Rust crate (dependency allowlist, clippy `disallowed-*`, cargo-deny, CI checks, capability patterns).
   - Grill: session boundaries and continuity (including host-owned spawning).
   - Wire blocking by topic, per the rules.
4. Run the OpenRouter provider prototype (#7). With the model call now a host callback, reframe it as an optional adapter. Then close #10 and run "Grill: build vs adopt" (#11).
5. Move closed prototype findings into the map's Decisions so far as tickets close. Fog to graduate later: async runtime and streaming, sessions (tree structure, storage), steering, built-in tools, compaction and skills, AGENTS.md, MCP, errors, testing, crate layout, MSRV, licence, release criteria.

## Housekeeping and gotchas

- **Stale remote branches:** `research/pi-sdk`, `research/openrouter-api`, `research/sans-io-rust`, `research/rust-llm-landscape` are fully merged into `main` but could not be deleted from the sandbox. Delete them: `git push origin --delete research/pi-sdk research/openrouter-api research/sans-io-rust research/rust-llm-landscape`.
- **Prototype branches** (kept as assets): `prototype/sans-io-host` (`prototypes/sans-io-host/`) and `prototype/rig` (`prototypes/rig/`). Never merged to `main`.
- Remote branch deletion and `gh` GraphQL are blocked by the sandbox proxy. Concurrent git operations on this repo are capped at two; on HTTP 429, wait and retry once.
- The map is edited by one writer at a time (the coordinating session) to avoid conflicts: resolvers post a comment and close, and the coordinator appends to Decisions so far.
- The `/wayfinder` command may not appear as a slash command in a session. The skill lives at `.claude/skills/wayfinder/SKILL.md`; read and follow it directly.
- Triage labels (`needs-triage`, etc.) exist in the repo but map tickets carry only `wayfinder:*` labels.
