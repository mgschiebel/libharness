# libharness handoff

Written 2026-11-01 (third session of the day). A Rust library embedding an LLM agent harness. Planning is driven by the `/wayfinder` skill (Matt Pocock's skills, installed in `.claude/skills/`). Repo: https://github.com/mgschiebel/libharness. No Rust crate exists on `main` yet; only research docs and planning. Prototypes live on branches.

## Start of a new session

1. Attach the repo with **push access from the start** (`mgschiebel/libharness`), clone it once (shallow is fine, give the clone a long timeout), then register the repo root so `CLAUDE.md` loads.
2. Read this file, then the map (issue #1). Read `.claude/skills/wayfinder/SKILL.md` directly; `/wayfinder` may not exist as a slash command.
3. **Check the in-flight ticket first** (see "Ticket status"): Research #29 was running as a background subagent when the last session ended and may not have finished.
4. Work through the map. Claim a ticket (assign it) before any work. Never resolve more than one ticket per session, except research tickets and AFK prototypes fired as subagents.

## Where the plan lives

- **Map:** GitHub issue #1, "Map: libharness first public release spec". It holds Destination, Notes (standing rules and leanings), Decisions so far, Not yet specified and Out of scope. Read it first. The map is edited by one writer, the coordinating session; resolvers post a comment and close, and the coordinator appends the Decisions line.
- Tickets are child issues of the map (native sub-issues, native blocking).
- `gh` GraphQL is blocked in these sessions. Use REST: `gh api repos/mgschiebel/libharness/...`. Sub-issues: `POST issues/1/sub_issues -F sub_issue_id=<issue id>`. Blocking: `POST issues/N/dependencies/blocked_by -F issue_id=<issue id>`. Open blockers: `GET issues/N/dependencies/blocked_by`.

## Destination

A written spec for the first public release of libharness, ready to hand off for implementation.

## User's standing rules (also in the map Notes)

1. Decisions are data driven. Research tickets must link primary sources.
2. **Rounds.** Per topic: research → prototype → recommend → grilling.
3. Grilling decides at the wider scope, from the recommendation.
4. Smart blocking: a grilling ticket is blocked by the recommend ticket on its own topic only.
5. Ask questions inline in chat as plain text. Never use menu or multiple-choice tools (voice-to-text user). Also in `CLAUDE.md`.
6. A research ticket closes only after its findings are on `main` under `research/` AND its `research/<name>` branch is deleted. The sandbox proxy cannot delete remote branches, so the human does it.
7. **Concepts only, no copied code** from other projects (rig, genai, swiftide, Pi), because of licence pollution and unverified licence compatibility.
8. Handoff and planning documents are plain Markdown files, not docs.

## The round model

Four roles, one label each: `wayfinder:research`, `wayfinder:prototype`, `wayfinder:recommend`, `wayfinder:grilling`. Written into the skill, `docs/agents/issue-tracker.md` and the map Notes.

- **Research (AFK)** gathers primary sources. Its last act is to plan the round: prototype tickets (each with a brief), one recommend ticket blocked by them, and the topic's grilling ticket blocked by the recommend ticket.
- **Prototype (AFK)** builds only. If it needs steering it parks (commit, comment, `awaiting-answer`, exit).
- **Recommend (HITL)** has two modes. *Triage:* takeable while any ticket in its round is `awaiting-answer`. *Decision:* all blockers closed; read every output, draft a recommendation, grill the human to a mutual topic-level recommendation, post it as the resolution comment, close. It may add research or prototype tickets inside its round, wired as blockers with a comment saying why.
- **Grilling (HITL)** decides at the wider scope and is the writer to the map (Decisions so far, graduating fog).
- **Clear-cut skip (revised 2026-11-01).** Grilling is the default and **the agent never raises the skip or asks whether a topic is clear-cut**. Only the human may invoke it, in decision mode, and only if every prototype in the round reported no surprises and nothing touches another topic. The recommend resolution comment lists the surprises found, or says "none". Reason: every round so far has turned up something unexpected, and topic-level sessions cannot see cross-topic effects.

How a grilling round is run (from the `grilling` skill): number the frontier questions, give a recommended answer for each, word each so "yes" accepts it, then wait.

## Ticket status

Closed so far: research #2 (Pi SDK), #3 (OpenRouter API), #4 (sans-IO in Rust), #5 (Rust LLM landscape), #18 (no-I/O enforcement); prototypes #6 (sans-IO host), #7 (provider abstraction over OpenRouter), #30 (provider trait against the Anthropic Messages API), #10 (rig), #19 (session-as-task), #22 (no_std gate), #23 (std enforcement stack), #26 (sans-IO core inside the session loop), #27 (no_std core in a std host); recommends #15 (build vs adopt), #24 (no-I/O enforcement mechanism).

Open:

| # | Ticket | State |
|---|--------|-------|
| 29 | Research: reuse vs write the OpenRouter adapter client | **IN FLIGHT, claimed.** Background subagent was running at session end. Added by the human because adapter client reuse (genai, async-openai) felt like fog. Its last act is to plan the round (prototypes, one recommend ticket, and an edge making #11 blocked by that recommend ticket) |
| 25 | Grill: no-I/O enforcement policy and crate layout | **takeable now, chosen as the next grilling.** Starts from the #24 recommendation |
| 20 | Recommend: session-as-task model | **takeable now**, decision mode; triage with the human already done |
| 11 | Grill: build vs adopt | **Open, partly grilled, unclaimed.** Blocked by #29 (and by the recommend ticket #29 plans). Closing is postponed by the human until that round has a recommendation |
| 16 | Recommend: provider abstraction | **takeable now**, decision mode; all prototypes (#7, #30) closed. Read #30's 9 leaks and 5 surprises first |
| 21 | Grill: session boundaries, continuity and spawning | blocked by #20 |
| 8 | Grill: host-interface and capability shape | blocked by #11 and #20 |
| 9 | Grill: provider abstraction | blocked by #11 and #16 |
| 28 | Parked: revisit wayfinder rounds export | not started; trigger in `docs/wayfinder-rounds/HANDOFF.md` is not met (no topic has finished grilling) |

### In-flight ticket: what to do first

#29 is assigned (claimed) to mgschiebel. It was fired as an AFK subagent in the previous session, which may have been stopped when that session ended. Its findings file `research/adapter-client-strategy.md` was already on `main` at last check, but the ticket was still open (no resolution comment, round not yet planned). Before anything else:

1. Read the ticket and its comments (`gh api repos/mgschiebel/libharness/issues/N` and `.../comments`). A resolution comment plus a closed state means it finished.
2. Look at what exists: branch `research/adapter-client-strategy` on origin, `research/adapter-client-strategy.md` on `main`, and whether the round's tickets exist. Remember to fetch with an explicit refspec.
3. If open and no live agent: resume with a fresh subagent from the findings and comments (the ticket body holds the brief). Its remaining job is the last act: plan the round (prototypes with briefs, one recommend ticket, the blocker edge making #11 wait for it), post the resolution comment, close. It stays claimed, so no other session will take it.
4. When #29 closes, check that it created its prototypes, the recommend ticket, and the blocker edge on #11. The human deletes the remote `research/adapter-client-strategy` branch once the findings are on `main` (the proxy cannot).

**Pending, non-blocking:** a live smoke test of the Anthropic integration against the real API, written by #30 and not run. It is a separate `live-smoke/` crate with three `#[ignore]`d tests; run from `prototypes/provider-abstraction` on branch `prototype/provider-abstraction-claude` with `ANTHROPIC_API_KEY=... cargo test --manifest-path live-smoke/Cargo.toml -- --ignored --nocapture`. The human can run it only after about a month (roughly early December 2026). #30 closed without it; if the live run disagrees with a fixture, the fixture is wrong. Remind the human when it is due and record the result.

## Findings in brief

Full detail is in each closed ticket and on the prototype branches (README in each prototype directory).

- **Pi:** two layers. Core is nested loops with hooks and steering/follow-up queues; the coding-agent layer adds append-only tree sessions, compaction and about 40 extension events.
- **OpenRouter:** OpenAI-shaped Chat Completions plus extras. SSE has comment lines to skip, and errors can arrive mid-stream inside an HTTP 200. Streaming tool-call delta format is unverified.
- **Crate landscape:** provider clients are well covered (genai, async-openai, rig-core). Agent loops only ship bundled with their own provider layer.
- **Build vs adopt (#15, recommended 2026-11-01):** build the core from scratch; no upstream contribution; concepts only, no copied code; whether the optional std-side OpenRouter adapter reuses genai or async-openai is left to the provider tickets. Grilling #11 runs.
- **Provider abstraction (#7, 2026-11-01):** the trait holds. `Provider` (request, decoder, one associated type) plus `Decoder` (push, finish): 4 methods, 1 associated type. OpenRouter 222 lines, an invented second integration 82, no trait change; 14 fixture tests (SSE comment lines, error mid-stream in a 200, split tool-call deltas, chunk and UTF-8 boundaries), not run against the live API. Streaming tool-call delta format verified from OpenRouter's OpenAPI spec (`https://openrouter.ai/openapi.json`). Leaks: streaming vs buffered invisible, tool-call fragments in the shared event type, error detail only in opaque metadata, provider options as unchecked JSON, retry has no home without a host timer. Surprises: the codec builds `no_std` + `alloc` (could sit in the gated core with only transport on the host); the callback's `String` error type is the real constraint (loses retry hints, partial output, cost; streaming has no slot). Branch `prototype/provider-abstraction`.
- **Provider trait vs Anthropic (#30, 2026-11-01):** the trait holds unchanged (4 methods, 1 associated type); Anthropic codec 386 lines, still `no_std` + `alloc`; 27 fixture tests pass (formats from Anthropic's official docs, not live). The shared types changed: `ModelRequest.system` added, and a `ReasoningBlock` (thinking, redacted) plus two stream-event variants, because thinking blocks must be replayed unchanged in a tool-use turn, so they are history (touches the session-history fog). Leaks: ordered content blocks flattened; tool results fold into one user message; errors only in opaque metadata; `retryable()` cannot tell a spend-cap 429 from a rate-limit 429; `FinishReason` lossy (`pause_turn`); usage cumulative, no cache counts; provider knobs as unchecked JSON; the callback's `String` error is still the binding constraint. Docs note: `Authorization: Bearer` now primary, `anthropic-version` `2026-10-21` latest (codec defaults to `2023-06-01`, configurable); official docs are fetchable with `curl` as `<page URL>.md` on platform.claude.com. Branch `prototype/provider-abstraction-claude`.
- **No-I/O enforcement (#24, recommended 2026-11-01):** `no_std` + `alloc` core gated by `cargo check -p <core> --target wasm32v1-none`, per package only (never workspace-wide; `default-members` set), plus `forbid(unsafe_code)`, a cargo-deny allowlist with a separate own-build-script check, and the zero-import wasm probe, all required in CI; allowlist and gate config changes need the human's review (CODEOWNERS). Costs accepted: `hashbrown`, `spin` (never held across `.await`), time via an injected clock capability, about 27 crates. Std denylist stack not built, kept as a documented fallback. Wasm component runtime stays fog. Grill #25 decides policy and crate layout, including where the codec sits.
- **Build vs adopt grilling (#11, in progress):** settled with the human: build the core from scratch; small crates are fine if they pass the allowlist (no agent framework or provider client in the core); core dependencies limited to permissive licences, enforced by cargo-deny; MCP stays host-side (registered as ordinary capabilities); a references note records which projects shaped which ideas. Written to `main`: `GLOSSARY.md` (Host, Library, Core, Capability, Session, Adapter) and `docs/adr/0001-build-the-core-from-scratch.md`. Open: whether the OpenRouter adapter reuses a client crate or is written from scratch (research #29 round); adapter placement goes to the crate-layout grilling (#25). The closing question and the map's Decisions line wait for that round.
- **Session-as-task (#19):** about 510 lines, serde and serde_json only, same 9 scenarios on tokio, futures-executor ThreadPool and LocalPool. Boxed `Send + Sync + 'static` callbacks; handle with abort, steer, follow-up; host-supplied JSON schema becomes a `final_answer` tool; spawn is an ordinary callback; `CancelToken` with sync hooks plus a "poll once more after cancel" guarantee; no timers in the library. Branch `prototype/session-task`.
- **Sans-IO core inside the loop (#26):** works (9 scenarios on two executors, 13 pure-logic tests with a fake clock), but costs more than guessed: library 513 to 719 lines, translation layer about 300 lines (not 35 to 90). The plain async loop is simpler to read; the sans-IO engine is simpler to test, though about 130 lines (abort final-poll, cancel hooks) still need an executor. The core could not express "the answer ends the run" (9-line patch), and steering diverged slightly (queued steers arrive one per turn, a steer before the first poll arrives a turn late). Branch `prototype/session-core`.
- **No-I/O research (#18):** no single mechanism proves no-I/O; layers do. rig-core, genai and swiftide fail a dependency-allowlist check. Findings: `research/no-io-enforcement.md`.
- **no_std gate (#22):** core builds `#![no_std]` + `alloc` + `forbid(unsafe_code)` under `cargo check --target wasm32v1-none` with serde, serde_json, schemars, thiserror, futures-core, tracing, hashbrown, spin. 27 external crates, 29-name allowlist, cold check 24 s. The CI script fails on every injected escape; probe wasm has zero imports. cargo-deny does not check build scripts in workspace members (separate script needed). Branch `prototype/no-io-nostd`.
- **std stack (#23):** `forbid` + generated clippy denylist (275 entries) + cargo-deny allowlist still misses 9 of 29 red-team escapes (unlisted std items, I/O in allowlisted deps, build scripts and proc macros, safe FFI wrappers, `clippy.toml` edits, test targets). Branch `prototype/no-io-std-stack`.
- **no_std core in a std host (#27):** works with real file and socket I/O on tokio and futures-executor; 64 parallel sessions fine, `spin` and `std` mutex throughput indistinguishable. The authoritative gate stays `cargo check -p nostd-core --target wasm32v1-none`; a native build with the host selected turns `std` on in the core's serde_json, and a workspace-wide wasm check fails. Branch `prototype/no-io-nostd-host`.

## Current design thinking (provisional until grilling decides)

- The library does **no I/O**; it reaches the outside world only through host-registered async callbacks (tools, skills, plugins, options, and the model call itself). Technically enforced from the start; the evidence so far favours the `no_std` + `alloc` gate over the std denylist stack.
- The host registers capabilities, then starts a **session** with a prompt; a session is an independent task with its own turn loop, running to a natural end or until the host aborts it. Sessions run in parallel. Sub-agents are child sessions started through a host-provided spawn callback.
- Library only; no host-side driving loop is in scope. User's working assumptions on continuity and spawning (sessions short-running, not persisted, host passes typed results and new prompts) are in the map Notes and go to Grill #21.

## Next steps

1. **Check the in-flight ticket #29** (see "In-flight tickets: what to do first").
2. **Grill: no-I/O enforcement policy and crate layout (#25).** Chosen as the next grilling because it is the only takeable grilling and starts from a settled recommendation (#24). Cross-topic questions to raise: where the codec and other pieces sit (gated core vs beside it, from #7), per-package gate and `default-members`, CODEOWNERS on gate config, and where the adapter lives (kept out of #29 on purpose). Every prototype in the #24 round reported surprises, so no clear-cut skip.
3. **Recommend: provider abstraction (#16).** Decision mode; all prototypes closed (#7, #30). Every prototype reported surprises, so no clear-cut skip. Cross-topic items to carry to Grill #9, #8 and #21: the callback's `String` error type and missing streaming slot, thinking blocks as session history, retry having no home without a host timer, and whether the codec sits in the gated core (Grill #25). Whether the adapter reuses a client crate comes from the #29 round, not here.
4. **Recommend: session-as-task model (#20).** Decision mode. Already accepted in triage: session-as-task shape, boxed Send callbacks (`?Send` is fog), cancel token with sync hooks plus the extra-poll guarantee, no timers in the library, host-supplied schema, streaming left unspecified. Remaining decision: the session engine, plain async loop versus sans-IO core inside it (evidence from #26). Note the new #7 finding that the model-call callback's `String` error type loses retry hints, partial output and cost, and that streaming has no slot in the callback shape; raise it here or in Grill #8. Then Grill #21 and Grill #8.
5. **After #29's round has its recommendation:** resume Grill build vs adopt (#11): the human's postponed closing question (confirm shared understanding), then write the map's Decisions line and close it. Then #8 (also needs #20) and #9 (also needs #16) become takeable once their blockers close.
6. Move decisions into the map's Decisions so far as grilling tickets close (grilling is the writer to the map). Fog to graduate later is listed in the map under Not yet specified.
7. After #16, Grill #9 also needs #11 closed. When a grilling ticket closes, check the pickup trigger in `docs/wayfinder-rounds/HANDOFF.md` and say so in the session if it is met (do not work it inside this map).
8. Remind the human about the pending Anthropic live smoke test around early December 2026.
9. Refresh the user's memory notes if they look stale (they predate tickets 18 to 30).

## Housekeeping and gotchas

- **Remote branches** now on origin: `main` and prototype branches `prototype/rig`, `prototype/sans-io-host`, `prototype/session-task`, `prototype/no-io-nostd`, `prototype/no-io-std-stack`, `prototype/no-io-nostd-host`, `prototype/session-core`, `prototype/provider-abstraction`. All prototype branches are kept as assets and never merged. Also on origin: `prototype/provider-abstraction-claude` (#30, kept as an asset). Expected from the in-flight ticket: `research/adapter-client-strategy` (#29; the human deletes it after its findings land on `main`; it may already exist). Worktrees from the previous session (`/home/claude/wt-*`) are sandbox-local and probably gone; recreate from the branches if needed.
- Remote branch deletion and `gh` GraphQL are blocked by the sandbox proxy. Concurrent git operations on this repo are capped at two; on HTTP 429 wait 5 seconds and retry once. Shallow clones: fetch with an explicit refspec (`git fetch origin <branch>:refs/remotes/origin/<branch>`).
- AFK prototype subagents work in their own `git worktree` off the main clone, never in the main checkout.
- `GLOSSARY.md` and `docs/adr/` now exist on `main` (created 2026-11-01 in the build-vs-adopt grilling): glossary terms Host, Library, Core, Capability, Session, Adapter (use them; avoid "provider" for the model call); ADR 0001 is "Build the core from scratch". Offer further ADRs only when hard to reverse, surprising and a real trade-off.
- GitHub `gh issue edit`/`close` use GraphQL and fail here; assign with `POST issues/N/assignees`, close with `PATCH issues/N -f state=closed`, release a claim with `DELETE issues/N/assignees`.
- Triage labels (`needs-triage`, etc.) exist in the repo but map tickets carry only `wayfinder:*` labels, plus the transient `awaiting-answer`.
