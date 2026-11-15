---
status: accepted
---

# Provider abstraction: codec modules, broker-first shared types and a structured model-call error

The library talks to a model service through codec modules in `libharness-codec`, one per wire format. The shared request, reply and error types follow what a model broker such as OpenRouter needs, and a direct provider stays only while it fits without changing them. Decided 2026-11-15 in the provider abstraction grilling, starting from the recommendation agreed with the human on 2026-11-12 (issue 16, from the prototypes in issues 7 and 30). It sits on ADR 0005 (the codec and the Exchange) and ADR 0006 (the model call is a plain-data callback).

## Considered options

- **A generic OpenAI-compatible module in the first release.** Rejected for now: the OpenRouter module already takes its base URL as plain data, so another OpenRouter-style endpoint works without a second module. It stays fog until a host needs one.
- **Typed provider options on the shared request.** Rejected: provider settings are one opaque, serialisable value, and each codec module defines and checks its own shape. The core never interprets a knob. Typed options stay fog.
- **The raw response body in the error.** Rejected: bodies can echo request data, and a request id and the provider's message are enough to find the call.
- **Guessing when `reasoning_details` fragments arrive with a missing or duplicate index.** Rejected: a wrong merge would be replayed verbatim to the provider on the next turn, so the codec fails with the partial reply instead.
- **A currency in the cost type.** Rejected: the unit of OpenRouter's `usage.cost` is not verified against the live API, so the type carries a bare decimal and the codec documents the unit.
- **Changing the shared types to fit Anthropic.** Rejected by the broker-first rule below. Anthropic passed without such a change: its three shared-type changes are broker needs too.

## What this settles

- **Codecs.** The first release ships two modules in `libharness-codec`: OpenRouter and Anthropic. The base URL is plain data on the codec (ADR 0006). The live smoke tests for both are required before the first public release (ADR 0005).
- **The trait is internal.** The `Provider`/`Decoder` pair (4 methods, 1 associated type across OpenRouter, an invented stub and Anthropic) is a contract inside `libharness-codec`. The host never sees it; it sees the model-level callback of ADR 0006.
- **Broker-first rule.** Shared types follow what a broker needs. A direct provider stays only if it fits without changing them, and is dropped otherwise.
- **Shared-type changes.** A system-prompt slot on the request. An opaque, provider-owned reasoning slot on replies and stream events, which the core only round-trips and ADR 0004's replay-verbatim rule keeps verbatim. Cache counts and an optional cost on usage. `pause_turn` stays in `Other`, with the raw reason as a codec detail.
- **Settings.** One opaque, serialisable value per request. Each codec module defines its shape and rejects a bad value with the structured error.
- **Reserved keys.** The reserved set is whatever keys the codec itself writes for that request. For OpenRouter: `stream`, `model`, `messages`, `tools`, `tool_choice` and any others set from shared request fields; Anthropic adds `system` and `max_tokens`. A settings value that collides is rejected before any network call, with an invalid-request error that names the key.
- **Structured model-call error.** On top of ADR 0006's base error (kind, retry hint, optional partial reply, optional usage): optional HTTP status, provider error type and code (strings), the provider's message (string) and a request id. The retry hint has three values: retry (with an optional delay from `Retry-After`), don't retry, and unknown. The codec sets it, because retryability is not a function of kind (a spend-cap 429 never succeeds on retry). The retry helper in `libharness-loop` treats unknown as don't retry. No raw response body.
- **Reasoning merge.** The OpenRouter codec merges streamed `reasoning_details` fragments by `index` into whole blocks, keeps unknown fields and replays them verbatim. A missing or duplicate index fails with a decode error carrying the partial reply. The rule is provisional, an assumption and not the spec, until the live OpenRouter smoke test.
- **Usage and cost.** `cost` is an optional decimal in the provider's own unit, with no currency in the type. Usage on the reply is final totals only, not cumulative stream updates.
- **Flattened content blocks.** The shared reply flattens ordered blocks, and the Anthropic encoder folds parallel tool results into one user message. Replay acceptance is assumed until the live smoke test; if a live result disagrees with a fixture, the fixture is wrong.

## Consequences

- Each codec reimplements its own error parsing (the decoder switches to error-body mode on status), which is why the trait stays at 4 methods.
- Provider knobs are checked in each module rather than in the type system, so a host gets a structured error at call time, not a compile error.
- Everything here is from fixtures and loopback mocks: 14 OpenRouter fixture tests, 27 Anthropic fixture tests (formats from the official docs), and nothing run against a live API. The Anthropic live smoke test (the human, around early December 2026, written by issue 30) and the OpenRouter one settle the replay assumption, the real tool-call fragmenting and interleaving, the unit of `usage.cost`, the `reasoning_details` merge and the TLS path.
- Not decided: typed provider options, a generic OpenAI-compatible module, and what the library stores as reasoning history beyond the opaque replay.
