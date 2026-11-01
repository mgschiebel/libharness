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
2. **Rounds.** The flow per topic is research → prototype → recommend → grilling (see "The round model" below). Research clarifies what to prototype, and its last act is to plan the round.
3. Grilling is only for identifying fog and for deciding, at the wider scope, from the recommendation.
4. Smart blocking: a grilling ticket is blocked by the recommend ticket on its own topic (or by the research ticket directly when no prototypes were worth building).
5. Ask questions inline in chat as plain text. Never use menu or multiple-choice tools (voice-to-text user). This is also in `CLAUDE.md`.
6. A research ticket closes only after its findings are rebased onto `main` under `research/` AND its `research/<name>` branch is deleted. The sandbox proxy cannot delete remote branches, so the human does it.
7. Handoff and planning documents are plain Markdown files, not docs.

## The round model

Agreed 2026-11-01. Written into the map Notes (rules 2 to 4), `.claude/skills/wayfinder/SKILL.md` (Rounds section) and `docs/agents/issue-tracker.md`.

Four roles, one label each: `wayfinder:research`, `wayfinder:prototype`, `wayfinder:recommend`, `wayfinder:grilling`.

1. **Research (AFK)** gathers primary sources. Its last act is to plan the round from the leading results: one prototype ticket per leading candidate per genuine uncertainty (skip any that would not change a decision), each with a brief (the question, what "validated" means, what to measure) and blocked by the research ticket; one recommend ticket blocked by all of those prototypes; and the topic's grilling ticket blocked by the recommend ticket. The number of prototypes is fog until research closes, so they are never pre-created. If nothing is worth prototyping, there is no recommend ticket and grilling is blocked by research directly.
2. **Prototype (AFK)** builds only. If it needs steering mid-build it parks: it commits to its branch, comments the question, adds the `awaiting-answer` label and exits. After the answer is posted a fresh subagent resumes it from the branch and the comments.
3. **Recommend (HITL)** is the human's interface to the round, with two modes on one ticket:
   - *Triage:* natively blocked, but takeable whenever any ticket in its round has `awaiting-answer`. Grill the human on those questions, comment the answers on the waiting tickets, remove the label, resume the subagents, release the claim. The ticket stays open.
   - *Decision:* all blockers closed. Read every output, draft a recommendation, grill the human to a mutual topic-level recommendation ("which candidate, and why"), post it as the resolution comment, close.
   - Either mode may create research or prototype tickets inside its round, wired as blockers of the recommend ticket with a comment saying why. That is also how a failed round gets a second batch.
4. **Grilling (HITL)** decides at the wider scope (across topics, fog), starting from the recommendation. It does not re-ask what was settled but can overturn it. It is the writer to the map: it appends to Decisions so far and graduates fog.

The agent recommends; only the human decides. **Clear-cut skip:** if the human (never the agent) declares a topic clear-cut in the recommend session, recommend writes the Decisions-so-far line itself and closes the grilling ticket with "resolved by recommend". The default is that grilling runs.

Because HITL reactions now live in recommend and grilling, an unreachable-user prototype is the normal case. Upstream's `prototype` skill already allows this ("if the user isn't reachable, default to ... and state the assumption") and is unchanged.

## Ticket status

| # | Ticket | State |
|---|--------|-------|
| 2 | Research: Pi SDK and agent-loop architecture | closed, `research/pi-sdk.md` |
| 3 | Research: OpenRouter API surface | closed, `research/openrouter-api.md` |
| 4 | Research: sans-IO design in Rust | closed, `research/sans-io-rust.md` |
| 5 | Research: Rust LLM crate landscape | closed, `research/rust-llm-landscape.md` |
| 6 | Prototype: sans-IO host interface | closed; branch `prototype/sans-io-host` |
| 7 | Prototype: provider abstraction over OpenRouter | open, unblocked, not started (AFK; brief added) |
| 10 | Prototype: rig against libharness requirements | open; built on branch `prototype/rig`. Under the round model its work is done; close it once the user agrees, which unblocks #15 |
| 15 | Recommend: build vs adopt | open, blocked by #10 |
| 16 | Recommend: provider abstraction | open, blocked by #7 |
| 11 | Grill: build vs adopt | open, blocked by #15; also blocks #8 and #9 |
| 8 | Grill: host-interface shape | open, blocked by #11 (its research and prototype, #2, #4, #6, are closed; no recommend ticket was needed) |
| 9 | Grill: provider abstraction | open, blocked by #11 and #16 |

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

0. The round model above is done: skill, tracker doc, map Notes, labels (`wayfinder:recommend`, `awaiting-answer`), recommend tickets #15 and #16, and the blocking rewiring. Still to decide: whether to close #10 (see table).
1. Get the user's answers on continuity and spawning, and their reaction to the rig prototype (#10) in the recommend ticket #15. The prototype's recommendation is to build from scratch.
2. Reshape the map: rewrite Notes and provisional leanings to the session-as-task model; reconsider the Pi-style "no sub-agents in core" leaning, which the session model reverses. Rename or reshape "Grill: host-interface shape" (#8) around it.
3. Add tickets:
   - Prototype: session-as-task with injected async capabilities, run on two executors to prove runtime neutrality.
   - Research: technical enforcement of "no I/O" in a Rust crate (dependency allowlist, clippy `disallowed-*`, cargo-deny, CI checks, capability patterns).
   - Grill: session boundaries and continuity (including host-owned spawning).
   - Wire blocking by topic, per the rules, using the round model: each new prototype gets a brief and a recommend ticket, and the grilling ticket is blocked by that recommend ticket.
4. Run the OpenRouter provider prototype (#7). With the model call now a host callback, reframe it as an optional adapter. Then close #10 and run "Grill: build vs adopt" (#11).
5. Move closed prototype findings into the map's Decisions so far as tickets close. Fog to graduate later: async runtime and streaming, sessions (tree structure, storage), steering, built-in tools, compaction and skills, AGENTS.md, MCP, errors, testing, crate layout, MSRV, licence, release criteria.

## Housekeeping and gotchas

- **Stale remote branches:** `research/pi-sdk`, `research/openrouter-api`, `research/sans-io-rust`, `research/rust-llm-landscape` are fully merged into `main` but could not be deleted from the sandbox. Delete them: `git push origin --delete research/pi-sdk research/openrouter-api research/sans-io-rust research/rust-llm-landscape`.
- **Prototype branches** (kept as assets): `prototype/sans-io-host` (`prototypes/sans-io-host/`) and `prototype/rig` (`prototypes/rig/`). Never merged to `main`.
- Remote branch deletion and `gh` GraphQL are blocked by the sandbox proxy. Concurrent git operations on this repo are capped at two; on HTTP 429, wait and retry once.
- The map is edited by one writer at a time (the coordinating session) to avoid conflicts: resolvers post a comment and close, and the coordinator appends to Decisions so far.
- The `/wayfinder` command may not appear as a slash command in a session. The skill lives at `.claude/skills/wayfinder/SKILL.md`; read and follow it directly.
- Triage labels (`needs-triage`, etc.) exist in the repo but map tickets carry only `wayfinder:*` labels.
