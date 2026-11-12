# PROTOTYPE (throwaway): OpenRouter adapter on `openrouter-rs`

Ticket: mgschiebel/libharness#32 ("Prototype: OpenRouter adapter on openrouter-rs"). Branch `prototype/openrouter-rs`, never merged to main.

**Question.** Can the optional OpenRouter adapter be built on `openrouter-rs` 0.23.0 (OpenRouter-specific typed SDK) and meet the OpenRouter streaming and field-fidelity needs behind a host-registered model-call capability? What do its release churn and runtime coupling force on the host?

**NOT exercised against the live API.** There is no API key. The mock is a loopback std-socket server replaying hand-built SSE fixtures, built from the documented formats and the OpenAPI schemas (`ChatStreamChunk`, `ChatStreamToolCall`, `ChatUsage`). They test my reading of the docs, not OpenRouter's real behaviour. The same author wrote the fixtures and the adapter, so a shared misreading would go unseen.

**Concepts only.** The crate's public API and docs were used. Its source was read to understand behaviour; nothing was copied from it or from any other project. The library-owned types (`src/model.rs`) and the `model_callback` shape come from our own provider-abstraction prototype.

**Versions.** `openrouter-rs = "=0.23.0"` exactly as in the ticket, default `reqwest` backend, the `chat().stream_unified(..)` path, pointed at the mock through the builder's `base_url`. No version or feature differed from the ticket (details of what was unexpected are under Surprises). Toolchain: rustc/cargo 1.97.0 (the crate's declared MSRV is 1.93, edition 2024).

## Run

```
cd prototypes/openrouter-rs
cargo test                              # 39 tests (25 protocol, 7 raw-stream, 7 runtime)
cargo test --features private-runtime   # +2 runtime tests for the workaround (41)
cd probe-wasip3 && cargo check          # fails on native by design (see Runtime)
cd probe-wasip3 && cargo check --target wasm32-wasip2   # passes
```

Environment change made for the wasip3 probe: `rustup target add wasm32-wasip2`.

## Layout

- `src/model.rs`: library-owned request, reply and error types (stand-ins for the core's; not counted as adapter glue).
- `src/adapter.rs`: the adapter glue. `OpenRouterAdapter::call` (structured error) and `model_callback` (session-task shape: `Fn(ModelRequest) -> Pin<Box<dyn Future + Send>>` returning `Result<ModelReply, String>`).
- `src/private_runtime.rs`: optional workaround (feature `private-runtime`): the adapter owns a tokio runtime so any executor can poll the callback future.
- `tests/common/mod.rs`: the mock server and fixtures. `tests/protocol.rs`: cases 1 to 8 plus extras, through the adapter. `tests/raw_events.rs`: what the crate's own unified stream yields, with no adapter. `tests/runtime.rs`: the runtime test.
- `probe-wasip3/`: a separate crate that enables only the `wasip3` backend.

## Results: cases 1 to 8

All through `chat().stream_unified(..)`. "Workaround" means the case passes only because the adapter compensates for something the crate does not do.

| # | Case | Result | Evidence and notes |
|---|---|---|---|
| 1 | SSE comment lines, also between events | **pass** | `:` lines before, between and inside an event, CRLF, a 4-byte emoji split across HTTP chunks, chunk sizes 1, 3, 17, 64 and whole. Caveats outside the case: a body that ends inside an event makes the crate panic (see Surprises 2), and a lone CR is not a line break. |
| 2 | Error chunk mid-stream inside a 200, text already delivered | **workaround (lossy)** | The documented shape (top-level `error` plus a choice with `finish_reason:"error"`) is decoded as an ordinary chunk and the top-level `error` is dropped. No `Error` event; the only signal is `finish_reason == "error"` on the terminal `Done`, so code, message and `error_type` are gone. Partial text is recoverable because the adapter accumulates text deltas itself. An error object alone in a frame (no envelope) does surface as `OpenRouterError::Api` with code, message and metadata. Details in Surprises 1. |
| 3 | Two parallel tool calls, fragments split mid-token and interleaved by `index`, `id`/`name` only on the first fragment | **pass** | The crate assembles by `index` into `Done.tool_calls`; `id`/`name` arriving in later fragments are merged too. A call that never has an `id` or a `name` is silently dropped from `Done` (the 0.9.0 behaviour is unchanged in 0.23.0); the adapter detects that by counting `ToolDelta` indexes and returns an error (workaround for that sub-case only). |
| 4 | Usage chunk with `cost`, `cost_details`, `is_byok` and a duplicated `finish_reason` | **pass** | All three reach `Done.usage` as typed fields; the duplicate finish is harmless; a usage chunk with `choices: []` also works. |
| 5 | Served `model` differs from the requested one | **pass** | `Done.model` is the last chunk's `model`; the request body kept the requested id. |
| 6 | Dropping the call future closes the server-side socket | **pass** | Mock saw the client close within about 20 ms of the drop. Not synchronous: reqwest's spawned connection task does the close, so the runtime must keep running afterwards. |
| 7 | `models`, `provider`, `reasoning`, `plugins` on the request | **pass** | All four reach the wire byte-for-byte equal to the input JSON, as typed request fields. No unchecked JSON passthrough exists in the crate. At the library boundary the request still carries them as `extra: serde_json::Value` (the callback's request must stay owned, serialisable data), and the adapter decodes each into the crate's typed struct. That decode rejects typos (`unknown field "ordre"`), where an unchecked passthrough would be silently ignored by OpenRouter. The flip side: a field the crate does not type has no way onto the wire. |
| 8 | HTTP 429 with `Retry-After`, and 402, as plain error responses | **pass** | `OpenRouterError::Api` with status, `api_code`, message, metadata and `retry_after` (seconds form: 7 survives; the HTTP-date form gives `None`). 402 is distinct and not retryable. A non-JSON 503 body comes through as the message. |

Result: 7 pass, 1 workaround (case 2); no outright fail.

Extras run alongside (all in `tests/protocol.rs` or `tests/raw_events.rs`):

| Extra | Result |
|---|---|
| Error chunk with envelope and `choices: []` | Decoded as an empty chunk, no error, no finish; adapter reports `Truncated`, code and message lost. |
| Text after the error chunk (the mock sends one; OpenRouter should not) | Delivered as normal content; indistinguishable from partial text because the unified stream has no per-chunk finish signal. |
| The crate does not stop at an `Error` event | Content after it keeps flowing; the adapter stops at the first `Error`. |
| HTTP 200 with a plain JSON error body (not SSE) | The crate sees an empty stream (no `data:` lines) and ends with an empty `Done`; adapter reports `Truncated`, error object lost. |
| Connection cut mid-stream | `Io`/`HttpRequest` error event ("error decoding response body"); adapter keeps the partial text. |
| Chunk missing `id`, `created`, `model` or `object` | Decode error (`missing field id`); the crate's chunk type requires the full envelope. |
| Body ends inside an SSE event (no blank line, or lone CR) | The crate's stream **panics**; adapter catches it with `catch_unwind` and keeps the text. |
| Fields the terminal `Done` does not carry | `provider`, `native_finish_reason`, `system_fingerprint`, `service_tier`, `openrouter_metadata` are decoded by the chunk type but not surfaced on `Done` (the raw `chat().stream()` exposes them). Reasoning (`reasoning`, `reasoning_details`) does reach events; the adapter drops it because the library reply has no slot. |

## Runtime test

Each scenario runs on its own OS thread so a panic or hang is classified rather than killing the test. Fixtures and mock as above.

| Scenario | Outcome |
|---|---|
| Build the adapter (`OpenRouterClient::builder().build()`) with no runtime | works |
| `LocalPool::run_until`, no tokio entered | **panic** at first poll: "there is no reactor running, must be called from the context of a Tokio 1.x runtime" |
| `ThreadPool` + `spawn_with_handle`, no tokio entered | **panic** (same message), carried back through the handle |
| `futures::executor::block_on`, no tokio entered | **panic** (same message) |
| `ThreadPool` + plain `spawn_ok`, no tokio entered | panic kills the worker; a pool of 1 never runs its next task |
| Inside tokio, `block_on` on a multi-thread runtime | pass |
| Inside tokio, `block_on` on a current-thread runtime | pass |
| `LocalPool` with a multi-thread tokio runtime merely entered (`rt.enter()`) | pass (tokio's workers drive the connection) |
| `LocalPool` with a current-thread tokio runtime entered but not driven | **hang** (the connection task is never polled) |
| Workaround: adapter-owned tokio runtime, polled by `LocalPool`, `ThreadPool`, `block_on`, no tokio entered | pass |
| Workaround: drop the callback future without polling it | task aborted, socket closed |

- **Does the `reqwest` backend work without tokio?** No. It panics at the first poll, when the connection is opened, not at construction. A host on a non-tokio executor gets a panic deep inside a session, not an error value.
- **Is `wasip3` usable on a native host?** No. With `default-features = false, features = ["wasip3"]` on x86_64 Linux the crate stops with its own `compile_error!` ("openrouter-rs needs an HTTP backend: enable the `reqwest` feature, or `wasip3` on a WASI target") followed by three follow-on type errors, because the backend is compiled only for `target_os = "wasi"`. On `wasm32-wasip2` the same probe passes `cargo check` (40.8 s, adds `wasip3`, `wit-bindgen`, `http-body-util`, `send_wrapper`). It was only checked, never run: no wasmtime here. So it does not help a native host; it matters only if the adapter itself runs as a Wasm component.
- **The workaround** (`src/private_runtime.rs`, 32 lines, feature `private-runtime`): the adapter owns a tokio runtime, spawns the whole call on it, and returns a `JoinHandle` future that any executor can poll. A plain `JoinHandle` does not cancel on drop, so a small `AbortOnDrop` wrapper is required for case 6. The spawn is eager: the request goes out even if the future is never polled. It adds no packages (tokio `rt-multi-thread` is already in the tree) but adds hidden worker threads and an adapter that must outlive its calls.

## Measurements

Host target (x86_64-unknown-linux-gnu), `cargo tree -e normal` plus `cargo metadata`, root excluded:

| Measure | Value |
|---|---|
| Unique packages | **114** (113 distinct names; one name appears in two versions) |
| Crates with `build.rs` | **16**: aws-lc-rs, aws-lc-sys, httparse, icu_normalizer_data, icu_properties_data, libc, mime_guess, proc-macro2, quote, ref-cast, rustls, serde, serde_core, serde_json, thiserror, zmij. `aws-lc-sys` compiles C. |
| Proc-macro crates | 11 |
| Same with `private-runtime` | identical (114 / 16) |
| Cold debug build (`cargo clean && cargo build`, lib only) | **77.8 s wall**, 106 s user + 15.6 s sys CPU. **Noisy:** three other builds ran on this 2-core machine at the same time (load average 2.1 rising to 4.0), so wall time is inflated. The research note measured 49 s for the crate alone on a quiet machine; treat 49 to 78 s as the range. |
| Adapter glue (`src/adapter.rs`, non-blank non-comment, rustfmt default) | **241 lines** (173 before `cargo fmt`, in the compact style the provider-abstraction prototype used). By function: `call` 85, `flatten` 26, `encode` 24, `convert_message` 21, `convert_call` 16, `new` 12, `take!` 12, `partial`/`finish`/`with_partial` 22, `model_callback` 8. About 35 of the 241 exist only to compensate for the crate (the `catch_unwind` guard, `finish_reason == "error"` and no-finish detection, dropped-tool-call detection, partial-reply bookkeeping). Plus 6 lines in `lib.rs` and, optionally, 32 for the private runtime. |
| Library-owned types (not glue) | `src/model.rs`, 110 lines |
| Tests and fixtures | about 930 lines of test code (including the mock), 39 tests (41 with the feature) |
| Public items the adapter touches | **70**: 20 types (OpenRouterClient, its builder, ChatClient, ChatCompletionRequest and builder, Message, Content, Plugin, ProviderPreferences, ReasoningConfig, Role, Tool, ToolCall, FunctionCall, UnifiedStreamEvent, ResponseUsage, ResponseCostDetails, OpenRouterError, ApiErrorContext, HttpRequestError), 20 functions and methods (`builder`, `base_url`, `api_key`, `build` x2, `chat`, `stream_unified`, `model`, `messages`, `tool`, `models`, `provider`, `reasoning`, `plugins`, `Message::new`, `assistant_with_tool_calls`, `tool_response`, `Tool::new`, `ToolCall::new`, `ChatCompletionRequest::builder`), 11 enum variants, 19 struct fields read. Three of the types are also coupled by their serde shape (`ProviderPreferences`, `ReasoningConfig`, `Plugin` are decoded from JSON; `ResponseCostDetails` is re-serialised). For scale the crate has roughly 550 public functions and 400 public structs and enums; the adapter uses a small corner of it, but a wide one in kind. |
| Last three minor releases that would have broken that surface | **1 of 3** (see below) |
| Cases passing | 7 pass, 1 workaround, 0 fail |
| Lost fields | see "What is lost" |
| JSON side channel needed | none into the crate for the four request fields; at the library boundary `extra: Value` stays (decoded into typed structs); none available out of the crate for the lost error object |
| Error-type flattening | see "Error flattening" |

### Release churn against that surface

Method: the CHANGELOG entry of each of the last three minor releases read against the surface above, plus an empirical check: the unchanged adapter source and tests were compiled against 0.22.0, 0.21.1, 0.21.0 and 0.20.0 in scratch copies (rustc reports all type errors in the crate, so errors that did not appear mean that part of the surface compiled unchanged).

| Release | Date | Breaks this surface? |
|---|---|---|
| 0.23.0 | 2026-10-30 | **Yes.** `UnifiedStreamEvent::Done` now carries assembled `tool_calls` and typed `usage` (before: no `tool_calls`, usage as raw JSON); the adapter's whole read side sits on that event. Also in the entry: `OpenRouterError` became `#[non_exhaustive]` (forces a wildcard arm), in-stream API errors changed from a serialization error to `Api`, `Role`/`ObjectType` gained `Other`. Compiling the adapter against 0.22.0, 0.21.x and 0.20.0 fails in exactly the `Done` pattern and the `usage` field reads, and nowhere else. |
| 0.22.0 | 2026-10-25 | No compile break on this surface (the only API changes are the per-choice `ErrorResponse.code` type and `ResponseCostDetails` fields becoming `Option`, which the adapter reads only as re-serialised JSON, so the JSON shape of `cost_details` could differ on null). |
| 0.21.0 | 2026-10-24 | No for this setup. It raised the MSRV from 1.85 to 1.93 (would break any pinned toolchain below 1.93; this repo pins 1.97) and put reqwest behind a default feature (breaks only `default-features = false` users). |

One release in three changed the stream event the adapter is built on, and the MSRV rose by eight minor Rust versions in another. The crate went from 0.4.7 to 0.23.0 in about 20 months; 13 minors between 0.10.0 (2026-05-20) and 0.23.0 (2026-10-30). The earlier 0.20.0 also moved reqwest 0.12 to 0.13 (breaks hosts that inject their own `reqwest::Client` through `http_client`, which this adapter does not).

### What is lost

Response side, relative to the wire:

- The top-level `error` object (code, message, `error_type`, `provider_code`) whenever the chunk also has `choices`. Unrecoverable through the crate: neither `stream_unified` nor the raw `stream()` has a field for it.
- Served `provider`, `native_finish_reason`, `system_fingerprint`, `service_tier`, `openrouter_metadata`, `alignment`, `safeguard_results`: decoded by the chunk type, not on the unified `Done`. Recoverable only by using the raw `chat().stream()` and doing the assembly in the adapter, which gives up the crate's assembler.
- Reasoning deltas and `reasoning_details` reach the unified events; the library reply here has no slot, so the adapter drops them. `prompt_tokens_details`, `completion_tokens_details` and `server_tool_use_details` are typed on usage; the library `Usage` here drops them (an adapter choice, not the crate's loss).
- A streamed tool call lacking an `id` or `name` is dropped by the crate with no signal.

Request side: `ToolResult.is_error` has no slot in OpenRouter's tool message (not the crate's fault); any request field the crate does not type cannot be sent.

### Error flattening

The callback's error type (`Result<ModelReply, String>` for comparability) has to flatten `OpenRouterError`, a `#[non_exhaustive]` enum, into one string. What the adapter's structured `ModelError` can keep from it: HTTP status, `api_code`, message, `retry_after` (seconds), metadata JSON (`error_type`/`provider_code` live only here, as untyped JSON). What the `String` loses: retryability, retry delay, status, metadata and the partial reply (text kept by the adapter, completed tool calls not). The structured shape also has to invent kinds the crate does not have: `Truncated` (the crate reports a cut stream as a normal end with no finish), a panic-as-`Protocol`, and a generic `Upstream` for `finish_reason:"error"` with no detail. The ADR leaves the callback's structured error type open; this adds weight to a small structured type owned by the core with kind, retry hint and partial reply.

## What the host is forced to take on

- **A tokio runtime in the process, or a hidden one.** With the default backend the call future panics on any other executor unless a tokio runtime that is actually being driven is entered. The alternative is the adapter's private runtime: extra threads, an eager-spawn call model, and a drop-aborts rule the adapter must own.
- **The HTTP and TLS stack, wholesale.** 114 packages, 16 build scripts including `aws-lc-sys` (a C toolchain), reqwest 0.13 and rustls with platform certificate verification (an up-to-date OS CA bundle, per the 0.20 notes). A host that wants its own transport cannot supply one: the only seam is `.http_client(reqwest::Client)`, which also pins the host to reqwest 0.13.
- **MSRV 1.93 and a crate on edition 2024**, and a pre-1.0 semver stream that changed the adapter's read side in one of the last three minors.
- **Compensation code for gaps the crate leaves**: a panic guard, error detection from `finish_reason`, dropped-tool-call detection, partial-text accumulation. About 15 per cent of the glue.
- **Blind spots the host cannot patch**: the error object on the documented mid-stream failure shape, the served provider, and any new OpenRouter request field the crate has not yet typed.
- **Default attribution**: the crate sends `X-Title: openrouter-rs` and `X-OpenRouter-Title: openrouter-rs` unless the host sets a title.

## Surprises

1. **The documented mid-stream error shape loses its error object.** The 0.23.0 breaking change ("an API error delivered inside a chat stream is now `OpenRouterError::Api`") applies only to a frame that fails to decode as a chunk. OpenRouter's documented failure is a chunk with a top-level `error` and a `choices` entry with `finish_reason:"error"`; that decodes as a normal chunk, the `error` key is ignored, and only `finish_reason:"error"` survives. The research listed the behaviour as unknown; running it shows the code, message and `error_type` cannot be recovered through the crate at all. The stream also does not stop at an error event, so text after it keeps flowing.
2. **The crate panics when the response body ends inside an SSE event.** A final `data:` line with no terminating blank line (or a lone CR line ending) is dispatched as an event and then the crate polls its already-finished line stream again, tripping a futures-util `Unfold` guard ("Unfold must not be polled after it returned `Poll::Ready(None)`"). The WHATWG rule is to discard an incomplete event. This is a panic inside the callback future, not an error value; the adapter needs `catch_unwind` around every poll. A truncated stream that closes cleanly mid-event would hit it.
3. **Dropping the future does not close the socket by itself.** The close is done by reqwest's spawned connection task. My first version of the cancel test blocked the runtime thread while waiting and failed; with an async wait it passed. A host that drops the future and then blocks the thread running the tokio runtime sees the connection stay open.
4. **A current-thread tokio runtime that is entered but not driven hangs** the call (no panic, no error), because the connection task never runs. A multi-thread runtime entered the same way works.
5. **`wasip3` alone is a hard compile error on a native host**, not a no-op (the README's "no effect on other targets" holds only when `reqwest` is also enabled).
6. **Default `X-Title`/`X-OpenRouter-Title: openrouter-rs` headers** go out on every request unless the host sets its own.
7. **Packaging mismatch, minor.** The 0.23.0 CHANGELOG says `src/client.rs` was split into `client/{mod,domains,management}.rs`; the published crate has a single 2383-line `src/client.rs`.

### Confirms expectations (not surprises)

- The default backend needs tokio at poll time; the README said so. It fails as a panic, not an error or a hang.
- 114 packages and 16 build-script crates, exactly the research figures; 49 s in the research against 78 s here is contention, not a difference.
- A tool call without `id` or `name` is still dropped silently (the 0.9.0 behaviour the ticket flagged).
- Typed `cost`, `cost_details`, `is_byok`, `models`, `provider`, `reasoning`, `plugins` all work, as the research read from source.
- High release churn (see the table above).
- Interleaved tool-call assembly, comment skipping, CRLF, split UTF-8, 429 `Retry-After` and 402 behave as the changelog and README say.

## Limits of this evidence

- Fixtures only; no live API. Real providers may fragment tool calls, order chunks or fail differently.
- One author wrote fixtures and adapter. The fixtures include the envelope fields (`id`, `created`, `model`, `object`) the crate requires; the earlier prototype's fixtures omitted them and would be rejected by this crate (real OpenRouter sends them).
- Cold-build time is noisy (three concurrent builds on 2 cores). Package and build-script counts are not.
- The release-churn check compiles the adapter against older versions rather than writing it against each; behavioural changes (as opposed to compile breaks) were read from the CHANGELOG, not run.
- `wasip3` was compiled for `wasm32-wasip2`, never executed.
- Shutdown of a private runtime from inside another tokio runtime's async context (tokio forbids dropping a runtime there) was not tested.
