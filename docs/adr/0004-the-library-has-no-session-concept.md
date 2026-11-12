---
status: accepted
---

# The library has no session concept; runs are the host's

The library knows State and one-call transitions. It has no session, no notion that a run ends, and no parent or child. How many times the host calls the core, and when it stops, is the host's: stopping is the host not calling again. Decided 2026-11-12 in the session boundaries grilling, from the human's view that a session is a host concern, on top of ADR 0003.

## Considered options

- **Keep "session" as the library's name for one run** (a State from a starting prompt to a natural end or an abort, with a handle, an inbox and a result callback). Rejected: once the loop is a host choice the library has nothing to end. The "end" was only the shipped loop's policy, and a shell existed only so an outside party could steer or abort a loop that owned the State.
- **Library-run spawning with parent and child sessions.** Rejected: a child is the host calling the library again with its own State, usually inside an ordinary tool callback. The library needs no parent/child concept, no depth or fan-out limit, and no inheritance rules.

## What this settles

- The glossary drops **Session**. Host, Capability and Loop no longer mention sessions. The word survives only as a host word.
- The core has no Done state, no End step and no abort transition. After each transition it returns the new State and what comes next (call the model, run these tools, or "the model replied and asked for nothing more"). The host or loop decides what that means.
- State is plain data the host can read and build. A prompt is one user message, and prior messages go in the same way. The core only appends to the history. Compaction stays host-side or stays open.
- State stores each provider's opaque reasoning block beside the assistant message it came with, and replays it verbatim. A host that wants it gone strips it from State.
- Continuity is entirely the host's. The library never persists anything and never builds the next prompt. Crash recovery is the host storing State and calling again; resume re-issues the call that was in flight (ADR 0003).
- Runs share no memory. The library has no globals and nothing held across runs, so a host can run it many times at once. Anything mutable belongs to exactly one run: its State, its inbox, its cancel token.
- Run policy leaves the core and belongs to the shipped loop: the steer and follow-up queues, the turn cap, the `final_answer` tool with the host's schema and validator, the bounded retry and the "finish by calling final_answer" nudge. A host-written loop does its own, or none. This narrows ADR 0003, where the core validated the typed result.
- The cancel contract from ADR 0003 (token with sync hooks, one extra poll, cancel wins over a late result) lives only in the optional async layer. The core has no cancel.
- The shell does not ship. The shipped loop is one async function taking the State, the capabilities, an inbox and a cancel token, and returning the State plus an outcome. The host keeps clones of the inbox and token to steer or abort. "A loop registered as a capability" goes away as a separate option: a host's own loop is just host code.
- The shipped loop, the cancel token, the inbox and the async helpers live in their own gated crate, `libharness-loop`, beside `libharness` and `libharness-codec` (amends the crate layout in ADR 0002).
- Cancel reaches children through linked tokens. The loop hands its token to every callback, and the token can make a linked child. A tool callback that starts another run gives it the child token. Steering a child is the host's job, through the child's own inbox or a tool the host registers.

## Consequences

- Spawning needs nothing from the library. The model reaches a child as an ordinary tool; the child's result is that tool's result; the parent waits because its tool call is pending; fan-out is several tool calls in one turn; depth and fan-out limits are the host's.
- No library-held capability registry shared across runs: how capabilities are registered and supplied per run is carried to the host-interface grilling.
- The `libharness` crate is the pure core and the model types. ADR 0002 still says it holds "sessions" and the capability registry; read that as amended here.
