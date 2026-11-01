# libharness handoff

Written 2026-11-01 (second session of the day). A Rust library embedding an LLM agent harness. Planning is driven by the `/wayfinder` skill (Matt Pocock's skills, installed in `.claude/skills/`). Repo: https://github.com/mgschiebel/libharness. No Rust crate exists on `main` yet; only research docs and planning. Prototypes live on branches.

## Start of a new session

1. Attach the repo with **push access from the start** (`mgschiebel/libharness`), clone it once (shallow is fine, give the clone a long timeout), then register the repo root so `CLAUDE.md` loads.
2. Read this file, then the map (issue #1). Read `.claude/skills/wayfinder/SKILL.md` directly; `/wayfinder` may not exist as a slash command.
3. Work through the map. Claim a ticket (assign it) before any work. Never resolve more than one ticket per session, except research tickets and AFK prototypes fired as subagents.

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

Closed so far: research #2 (Pi SDK), #3 (OpenRouter API), #4 (sans-IO in Rust), #5 (Rust LLM landscape), #18 (no-I/O enforcement); prototypes #6 (sans-IO host), #10 (rig), #19 (session-as-task), #22 (no_std gate), #23 (std enforcement stack), #26 (sans-IO core inside the session loop), #27 (no_std core in a std host); recommend #15 (build vs adopt).

Open:

| # | Ticket | State |
|---|--------|-------|
| 24 | Recommend: no-I/O enforcement mechanism | **takeable now**, decision mode, all prototypes closed |
| 20 | Recommend: session-as-task model | **takeable now**, decision mode; triage with the human already done |
| 11 | Grill: build vs adopt | **takeable now**; starts from the #15 recommendation; also blocks #8 and #9 |
| 7 | Prototype: provider abstraction over OpenRouter | AFK, unblocked, not started (brief added) |
| 16 | Recommend: provider abstraction | blocked by #7 |
| 21 | Grill: session boundaries, continuity and spawning | blocked by #20 |
| 25 | Grill: no-I/O enforcement policy and crate layout | blocked by #24 |
| 8 | Grill: host-interface and capability shape | blocked by #11 and #20 |
| 9 | Grill: provider abstraction | blocked by #11 and #16 |

Nothing is claimed at the time of writing.

## Findings in brief

Full detail is in each closed ticket and on the prototype branches (README in each prototype directory).

- **Pi:** two layers. Core is nested loops with hooks and steering/follow-up queues; the coding-agent layer adds append-only tree sessions, compaction and about 40 extension events.
- **OpenRouter:** OpenAI-shaped Chat Completions plus extras. SSE has comment lines to skip, and errors can arrive mid-stream inside an HTTP 200. Streaming tool-call delta format is unverified.
- **Crate landscape:** provider clients are well covered (genai, async-openai, rig-core). Agent loops only ship bundled with their own provider layer.
- **Build vs adopt (#15, recommended 2026-11-01):** build the core from scratch; no upstream contribution; concepts only, no copied code; whether the optional std-side OpenRouter adapter reuses genai or async-openai is left to the provider tickets. Grilling #11 runs.
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

1. **Recommend: no-I/O enforcement mechanism (#24).** Decision mode. The evidence leans to the `no_std` gate (#22, #27) with the std stack (#23) as a fallback. Every prototype reported surprises, so no clear-cut skip. After it, Grill #25 (policy and crate layout).
2. **Recommend: session-as-task model (#20).** Decision mode. Already accepted in triage: session-as-task shape, boxed Send callbacks (`?Send` is fog), cancel token with sync hooks plus the extra-poll guarantee, no timers in the library, host-supplied schema, streaming left unspecified. Remaining decision: the session engine, plain async loop versus sans-IO core inside it (evidence from #26 above). Then Grill #21 and Grill #8.
3. **Grill: build vs adopt (#11)**, starting from the #15 recommendation.
4. **Run #7** (OpenRouter provider prototype, AFK, reframed as an optional std-side adapter). Needs a free slot: at most two concurrent git operations on this repo; use `git worktree` from the existing clone instead of cloning. It unblocks Recommend #16.
5. Move decisions into the map's Decisions so far as grilling tickets close. Fog to graduate later is listed in the map under Not yet specified: runtime and streaming, `?Send` flavour, capability registry, session history, mid-run control, built-in tools, context features, MCP, errors, testing, crate layout, MSRV, licence, release criteria.
6. Refresh the user's memory notes if they look stale (they predate tickets 18 to 27).

## Housekeeping and gotchas

- **Remote branches** now on origin: `main` and prototype branches `prototype/rig`, `prototype/sans-io-host`, `prototype/session-task`, `prototype/no-io-nostd`, `prototype/no-io-std-stack`, `prototype/no-io-nostd-host`, `prototype/session-core`. All prototype branches are kept as assets and never merged. No stale `research/*` branches remain.
- Remote branch deletion and `gh` GraphQL are blocked by the sandbox proxy. Concurrent git operations on this repo are capped at two; on HTTP 429 wait 5 seconds and retry once. Shallow clones: fetch with an explicit refspec (`git fetch origin <branch>:refs/remotes/origin/<branch>`).
- AFK prototype subagents work in their own `git worktree` off the main clone, never in the main checkout.
- `GLOSSARY.md` and `docs/adr/` do not exist yet; domain-modeling creates them lazily. No ADR has been offered yet.
- Triage labels (`needs-triage`, etc.) exist in the repo but map tickets carry only `wayfinder:*` labels, plus the transient `awaiting-answer`.
