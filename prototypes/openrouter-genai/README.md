# PROTOTYPE (throwaway): OpenRouter adapter on genai

Ticket: mgschiebel/libharness#31. Branch `prototype/openrouter-genai`, never merged to main.

**Question.** Can the optional OpenRouter adapter be built on **genai** (0.7.2, `open_router::` namespace) and still meet the OpenRouter streaming and field-fidelity needs behind a host-registered model-call capability? What has to be translated, what is lost, and what does it force on the host (tokio, reqwest, build weight)?

**Short answer.** Yes, with glue. Every numbered case passes or passes through a workaround, apart from one sub-case of case 2 (HTTP 200 whose whole body is a plain JSON error), which fails because genai drops the body. Two things carry most of the weight: genai's raw-frame sink (a public `ChatOptions` hook, not mentioned in the earlier research) recovers `cost`, the served model and `reasoning_details`, and `ChatOptions::extra_body` carries `models`, `provider`, `reasoning` and `plugins` as unchecked JSON. The price is a mandatory tokio runtime (without one, the call panics at TCP connect), reqwest 0.13 with rustls and `aws-lc-sys`, and 145 packages.

**NOT exercised against the live API.** No API key; the server is a loopback mock on std sockets that replays hand-built SSE. The fixtures are built from the documented formats and OpenRouter's OpenAPI spec (`ChatStreamChunk`, `ChatStreamToolCall`, `ChatUsage`), so they test my reading of the docs, not OpenRouter's real behaviour. **Concepts only:** genai's source was read to understand behaviour; no code was copied from it or from any other project. The library-owned types and the fixtures are adapted from our own provider-abstraction prototype.

## Run

```
cd prototypes/openrouter-genai
cargo test                      # 30 tests: tests/cases.rs (23), tests/runtime.rs (7); all pass
cargo test --test runtime -- --nocapture --test-threads=1   # prints one RUNTIME line per scenario
```

Toolchain: cargo/rustc 1.97.0 (same as the research). genai is pinned `=0.7.2`; **0.7.4 was already on crates.io** when this ran and was not tested.

## Layout

- `src/types.rs` (90 code lines): library-owned `ModelRequest`, `ModelReply`, `ModelError` (same shape as the provider-abstraction prototype's std side). Not glue; it would live in the core.
- `src/lib.rs` (264 code lines): the adapter. `OpenRouterGenai::new(Config)`, `.complete(req)`, `model_callback(adapter)` returning `Fn(ModelRequest) -> Pin<Box<dyn Future<Output = Result<ModelReply, String>> + Send>>` (the session-task callback shape).
- `tests/support/mod.rs`: the mock server (215 lines): scripted status, headers and chunked body; records requests; reports when the client closes a hanging connection.
- `tests/fixtures.rs`, `tests/cases.rs`, `tests/runtime.rs`.

How it uses genai (public API only): `Client::builder().with_auth_resolver_fn(..).with_service_target_resolver_fn(..)` sets the key and the base URL; `exec_chat_stream("open_router::<id>", ChatRequest, Some(&ChatOptions))` with `capture_usage`, `capture_content`, `capture_tool_calls`, `extra_body`, and `with_raw_frame_fn` (the raw-frame sink). The loop folds `Chunk` events into text, takes tool calls, usage and stop reason from `End`, and stops at the first error item.

## Results (cases 1 to 8 of the shared protocol)

| # | Case | Result | How |
|---|---|---|---|
| 1 | SSE comment lines, also between events | **pass** | Skipped by genai at all 7 chunk sizes (1, 2, 3, 5, 17, 64, whole), including CRLF endings and a 4-byte emoji split across chunks. |
| 2 | Error chunk mid-stream inside HTTP 200, text already delivered | **workaround** | genai yields `Err(Error::ChatResponse { body })` where `body` is only the `error` object (code, message, metadata with `error_type` and `provider_code`). No partial text comes with it and no `End` follows, so the partial text is recoverable only because the adapter accumulates `Chunk` events as they arrive. genai keeps yielding after the error item (a later `content` chunk still comes out), so the adapter must stop at the first error. Sub-cases below. |
| 3 | Two parallel tool calls, fragments split mid-token, interleaved by `index`, `id`/`name` only on first fragment | **pass** | Folded correctly by index at every chunk size; `End.captured_content` has both calls with parsed arguments. Unparsable or empty arguments are left as a string, so `args_error` and the empty-to-`{}` rule are adapter code. |
| 4 | Usage chunk with `cost`, `cost_details`, `is_byok`, duplicated `finish_reason` | **workaround** | Tokens (prompt, completion, total) and the duplicate finish are fine through typed events. genai's `Usage` has no cost field (checked by round-tripping `cost` and `is_byok` through its serde type: both vanish), so `cost`, `cost_details` and `is_byok` come from the raw-frame sink. |
| 5 | Served `model` differs from the requested one | **workaround** | Not in any typed event (the serialised `End` has no trace of it). Read from the `model` key of raw frames. |
| 6 | Dropping the call future closes the server-side socket | **pass** | Server sees the close within the 5 s window after the drop; a control test shows it stays open while the future lives. With the adapter-owned runtime (below) the spawned task needs an abort-on-drop wrapper (a dropped `JoinHandle` detaches rather than cancels); with it, the same test passes. |
| 7 | Request with `models`, `provider`, `reasoning`, `plugins` | **workaround** (all four reach the wire; none typed) | Only as unchecked JSON through `ChatOptions::extra_body`, merged at the top level of the body. See the exact JSON below. |
| 8 | HTTP 429 with `Retry-After`, and HTTP 402, as plain error responses | **pass** | Status, body and headers survive (`Retry-After: 7` read as 7); 402 maps to a non-retryable credits error; a non-JSON 503 body maps to upstream. They surface as a stream item after `Start`, not from `exec_chat_stream` (see Surprises). |

Case 2 sub-cases, all in `tests/cases.rs`:

| Sub-case | Result |
|---|---|
| Error chunk as the only event | pass (`Err(ChatResponse)`) |
| `finish_reason: "error"` with no `error` object | workaround: genai ends normally with `StopReason::Other("error")`; the adapter turns it into a failure |
| Stream cut off mid-JSON | pass: `Err(StreamParse)`, partial text kept |
| Stream ends cleanly with no finish reason and no `[DONE]` | workaround: genai ends silently with no `End`; the adapter reports a truncation |
| HTTP 200 whose entire body is a JSON error object (`{"error":{"code":402,...}}`) | **fail**: the stream is `Start` then nothing; no error, no `End`, and the sink sees one empty frame. The 402 message is lost and the adapter can only say "truncated". Not recoverable through any public hook I found. |

### The JSON side channel (case 7)

Exactly what the adapter hands to `extra_body`, and what appeared on the wire (the mock's recorded request body, with genai's own fields):

```json
{
  "models": ["anthropic/claude-x", "openai/gpt-4o-mini"],
  "provider": {"order": ["anthropic"], "allow_fallbacks": false},
  "reasoning": {"effort": "low", "max_tokens": 128},
  "plugins": [{"id": "web", "max_results": 2}]
}
```

- All four keys arrived verbatim at the top level, next to genai's `model`, `stream: true`, `messages`, `tools` and `stream_options: {"include_usage": true}`. genai always writes `model`, so a `models`-only request is not possible.
- `extra_body` is an unchecked merge: `{"model": "evil/override", "stream": false}` replaced the adapter's own `model` and `stream`, and a misspelt key (`reasonning`) was forwarded unchanged (OpenRouter would ignore it silently).
- genai's typed `ChatOptions::with_reasoning_effort` writes OpenAI's top-level `"reasoning_effort": "low"`, not OpenRouter's `reasoning` object. Whether OpenRouter honours the former is **unknown** (not checked), so `reasoning` also goes through `extra_body`.
- Response side: `cost`, `cost_details`, `is_byok` and the served `model` need the raw-frame side channel (the sink callback; it receives each frame's data string, and the adapter parses it a second time as JSON).

## Runtime test

Each scenario runs on its own thread with an 8 s watchdog so panic, error and hang are told apart (`tests/runtime.rs`; the mock runs on plain std threads). Cases 1 to 8 themselves all ran inside a tokio runtime (`#[tokio::test(flavor = "multi_thread")]`).

| Scenario | Outcome |
|---|---|
| `futures-executor` `LocalPool`, no tokio runtime entered | **panic**: "there is no reactor running, must be called from the context of a Tokio 1.x runtime" |
| `futures-executor` `ThreadPool`, no tokio runtime entered | **panic**, same message (the task is dropped; the result channel disconnects) |
| Awaited on a tokio task | ok |
| `LocalPool`, with a tokio runtime elsewhere and its `Handle` entered on the driving thread | ok |
| `ThreadPool`, tokio `Handle` entered on each worker (`after_start`) | ok |
| `LocalPool`, no tokio, adapter-owned runtime (workaround) | ok |
| `ThreadPool`, no tokio, adapter-owned runtime (workaround) | ok |

- Without tokio it is a panic, not a hang and not an error value. The backtrace puts it in reqwest's connector (hyper-util `HttpConnector` creating a tokio `TcpStream`), on the first poll, before any genai timer is reached; genai's own `tokio::time::timeout` calls on the stream path were therefore never exercised without a runtime.
- Two ways to live with it: the host enters a tokio runtime context on every thread that polls the future, or the adapter owns a runtime and spawns each call on it (`with_owned_runtime`, 34 glue lines). The second keeps the callback runtime-neutral from the host's side at the cost of an extra runtime and thread(s) in the process, plus two traps found on the way: the `JoinHandle` must be aborted on drop (case 6), and dropping the adapter inside async code panics unless the runtime is shut down with `shutdown_background` (in the `Drop` impl).

## Measurements

| What | Value |
|---|---|
| Unique host-target packages (`cargo tree -e normal`, excluding the root) | **145**, identical to the research's genai probe; the adapter's own extra dependencies (`serde_json`, `futures-util`, `tokio`) add 0 because genai already pulls them in |
| Packages with `build.rs` (`cargo metadata` intersected with the host tree) | **17**: `aws-lc-rs`, `aws-lc-sys`, `crc32fast`, `encoding_rs`, `getrandom`, `httparse`, `icu_normalizer_data`, `icu_properties_data`, `libc`, `mime_guess`, `proc-macro2`, `quote`, `rustls`, `serde`, `serde_core`, `serde_json`, `zmij` |
| HTTP/TLS stack in the tree | reqwest 0.13.5, hyper 1.12.0, h2, tokio 1.54.1, rustls 0.23.46 with `aws-lc-rs`/`aws-lc-sys` (C code) |
| Cold debug build (`rm -rf target; cargo build`, 2 cores, registry cached) | **83 s** (first run), **117 s** (second run, load average rose to 4). **Noisy: three other builds were running on this machine at the same time**, so treat as a range; the research measured 50 s for the same dependency set on a quieter machine |
| Adapter glue (`src/lib.rs`, code lines, no tests or fixtures) | **264**, of which: setup and callback 42, call loop and reply assembly 89, raw-frame side channel 35, request mapping 25, error flattening 39, adapter-owned runtime 34 |
| Library-owned types (`src/types.rs`) | 90 code lines (not glue) |
| Tests | cases 326 lines (23 tests), runtime 145 (7), mock server 215, fixtures 54 |
| Cases passing | 1, 3, 6, 8 pass; 2, 4, 5, 7 pass through a workaround; none fail outright, but one sub-case of 2 (HTTP 200 plain-JSON error body) fails |
| For comparison (research, not re-measured) | provider-abstraction prototype's whole OpenRouter codec 222 lines with 4 external crates |

### Fields lost or needing a side channel

| Field | Typed genai path | Recovered by |
|---|---|---|
| `usage.cost` | lost | raw-frame sink |
| `usage.cost_details`, `usage.is_byok` | lost | raw-frame sink |
| served `model` | lost | raw-frame sink |
| streamed `reasoning_details` (signed or encrypted blocks) | lost (typed output has only `reasoning` text via `ReasoningChunk` and `captured_reasoning_content`) | raw-frame sink (test: `reasoning_details_survive_only_through_the_side_channel`); **echoing them back on the next request through genai's streaming path was not tested** |
| error chunk's `finish_reason: "error"` choice, `native_finish_reason`, `system_fingerprint`, `service_tier`, `openrouter_metadata` | lost | raw-frame sink if wanted (not extracted here) |
| `ToolResult.is_error` | no slot (also none on OpenRouter's `tool` message) | none |
| body of an HTTP 200 plain-JSON error | lost | none found |
| `models`, `provider`, `reasoning`, `plugins` (request) | untyped | `extra_body` JSON |

### What the callback's error type has to flatten

genai's `Error` has about 30 variants, none `#[non_exhaustive]`, and its public variants carry reqwest types (`StatusCode`, `HeaderMap`). The adapter maps `ChatResponse` (error chunk), `HttpError` (directly or boxed inside `WebStream`), `StreamParse`, truncation, and everything else, onto 8 kinds with status, code, retry-after, metadata and partial reply (`ModelError`). The session-task callback returns `Result<_, String>`; flattening to it loses all of that: the test shows a mid-stream 429 arriving as `"RateLimited: Rate limit exceeded"`, with retryability, `Retry-After`, the code, `error_type` metadata and the partial text gone. Same conclusion as the provider-abstraction prototype: the callback's error type has to become a small structured type owned by the core, or retry has to live inside the adapter (which needs a timer the library does not have).

## What the host is forced to take on

- **A tokio runtime**, non-optional: reqwest's async client needs one at call time (panic otherwise, see above). The host either enters a runtime context on whatever executor drives the session, or accepts an adapter-owned runtime.
- **reqwest 0.13 and its stack**: hyper, h2, rustls with `aws-lc-sys` (a C build), 145 packages and 17 build scripts in total. TLS licences beyond MIT/Apache apply (see the research, section 2).
- **genai's public types leak reqwest**: `genai::Error::HttpError` holds `StatusCode` and `HeaderMap`. A host that passes its own `reqwest::Client` (`with_reqwest`) must match genai's reqwest major.
- **Every provider adapter in genai ships along**; only the whole client is reusable (the codec layer is private).
- **A moving target**: genai is pre-1.0 (0.7.4 is already out), and the behaviours this adapter depends on (error-item semantics, stream continuing after an error, the sink API) are not covered by any stability promise I found.
- **Dependencies that cannot sit in the no-I/O gate** (per the earlier research); fine for the adapter, which sits outside it by ADR 0002.

## Surprises

1. **The raw-frame sink exists in 0.7.2 and the research did not mention it.** `ChatOptions::with_raw_frame_fn` / `ChatFrameSink` (documented as an escape hatch for billing details) is what makes cost, `cost_details`, `is_byok`, the served model and streamed `reasoning_details` reachable at all. Without it, cases 4 and 5 would be fails and "where `cost` can be read" would have no answer. Cost: every frame is JSON-parsed twice (genai, then the sink), and the sink must not block.
2. **genai keeps yielding after an error item.** After the mid-stream error, a later content chunk still came out of the stream, and the stream then ended with no `End`. The research said genai "turns an `error` key into an error value", which is true, but a caller that does not stop at the first error will happily append post-error text.
3. **A 200 with only a plain JSON error body vanishes.** The stream is `Start` and then nothing: no error value, no `End`, and the sink sees one empty frame. This is the one documented OpenRouter failure shape that genai silently drops.
4. **Without tokio it panics inside reqwest's TCP connect, not at genai's timers.** The research flagged genai's `tokio::time::timeout` on the stream path as the risk; the first failure actually happens earlier, in the connector. It is a panic on first poll, so a host on `futures-executor` finds out immediately rather than hanging.
5. **HTTP errors arrive as a stream item after `Start`, and doubly wrapped.** `exec_chat_stream` returns `Ok` for a 429; the error is `WebStream { error: Box<dyn Error> }` whose inner value is `genai::Error::HttpError`, so reading status and `Retry-After` needs a `downcast_ref`.
6. **`ToolCallChunk` events carry accumulated arguments, not deltas**, as a JSON string that only becomes an object at `End`; unparsable arguments stay a string with no error flag.
7. **`extra_body` can overwrite `model` and `stream`**, and genai's typed reasoning option emits OpenAI's `reasoning_effort`, not OpenRouter's `reasoning` object.
8. **Small wire additions by genai:** every tool gets `"strict": false`, `stream_options.include_usage: true` is always sent (OpenRouter documents it as deprecated and a no-op), and an assistant message that only has tool calls goes out with `"content": ""` instead of null. None broke the mock; whether OpenRouter minds is unknown.
9. **An owned tokio runtime panics if dropped inside async code**, and a dropped `JoinHandle` does not cancel. Both bit the adapter-owned-runtime workaround and are fixed in it (`shutdown_background`, abort-on-drop).
10. **Build time was higher than the research's 50 s** (83 s and 117 s), most likely because three other builds shared the 2 cores; it is not evidence that the dependency set changed (the package and build-script counts match exactly).
11. **genai 0.7.4 is already published** (the research and this prototype use 0.7.2); not tested, so none of the behaviours above is confirmed for it.

### Things that merely confirm the research's expectations

- genai needs tokio and reqwest unconditionally: 145 packages and 17 build scripts, as probed.
- Interleaved parallel tool-call deltas fold correctly by index after the 0.7.1 fix.
- The `error` key becomes an error value.
- Comment lines are skipped; the duplicated `finish_reason` and usage-chunk shape are handled; token counts are captured.
- genai's `Usage` has no cost field, and the served model is not exposed by the typed events.
- Streamed `reasoning_details` are not in the typed output (consistent with the open upstream PR).
- `extra_body` is the only path for `models`, `provider` and `plugins`.
- HTTP error responses keep their headers, so `Retry-After` is readable.
- Dropping the call future closes the socket (the stream owns the connection).

## Not covered

Live API behaviour; real per-provider tool-call fragmenting; whether OpenRouter honours `reasoning_effort`; round-tripping `reasoning_details` into the next request; genai's own timers without a runtime (the panic comes first); genai 0.7.4; the `bedrock-sigv4`, `native-tls` and `otel` features; non-default builds that might drop TLS (genai exposes `rustls-tls` and `system-proxy` as default features only, and reqwest stays mandatory).
