---
status: accepted
---

# The core is one model call with state in and out; the loop is a host choice

The library's essential shape is a set of pure transitions at the centre of the core: one model call per step, with State going in and coming out as owned, serialisable data and nothing kept between calls. The Loop that drives those transitions is not fixed by the library. A host can write its own, register one as a capability, or use the one loop the library optionally ships. An earlier shape, where the library owns the turn loop and the host only starts sessions, survives as that optional shipped loop. Decided 2026-11-12 in the session-as-task recommendation, from the human's reframe in the provider-abstraction recommendation and the evidence of the single-call-core prototype.

## Considered options

- **Library-owned loop (session-as-task).** Tested twice (session-task, and the sans-IO core inside it) and it works. Rejected as the essential shape because it makes every host take the library's loop, and the layering below it costs little: a host loop is 13 lines at minimum, a registered loop 22 to 34, the shipped loop 44.
- **The earlier sans-IO core as the lower layer.** Rejected: it cannot build `no_std` (79 errors on `wasm32v1-none`) and its state is not serialisable. The pure transitions were rewritten in the session's own types instead, which also removed the roughly 300-line translation layer that retrofitting it needed.

## What this settles

- The pure transitions are the session engine. They take no clock and no callbacks; State carries the host-supplied id.
- The cancel contract has three parts and the library ships all three in its async helpers: a token with synchronous hooks, one extra poll after cancel, and cancel winning over a result that lands in that extra poll. Without the third part a loop that finishes a tool batch during the extra poll carries on after an abort.
- State is the checkpoint. A host can store it and resume, which re-issues the model call that was in flight; the library makes no exactly-once promise for that call.
- The typed result is checked by a synchronous validator the host supplies, together with the JSON schema. The core does no schema validation.
- The clock capability from ADR 0002 is required only where something needs it (a shipped loop, an adapter, a host budget check). A host-owned loop need not register one.

## Consequences

- Whether "session" is a library concept at all, and whether the handle-and-inbox shell around the shipped loop ships, was left open for the session boundaries grilling. Settled by ADR 0004: there is no session concept, the shell does not ship, and the typed-result validation, turn cap and queues move from the core to the shipped loop, which narrows the "What this settles" list above (the core validates nothing; "a loop registered as a capability" is no longer a separate option).
- A plain loop cannot preempt an uncooperative blocking call without a second thread or a cancel-aware client. That cost stays with the host.
- The model request carries the whole history on every call, so request size grows quadratically with turns. That comes from the request, not from this layering.
