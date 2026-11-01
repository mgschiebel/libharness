# PROTOTYPE (throwaway): provider abstraction over OpenRouter

Ticket: mgschiebel/libharness#7. Branch `prototype/provider-abstraction`, never merged to main.

**Question:** does a small provider trait hold with OpenRouter first and a second, differently shaped integration behind it, and what leaks? Under session-as-task (`prototype/session-task`) the model call is a host-registered callback `Fn(ModelRequest) -> Future<Result<ModelReply, String>>`, so this is an OPTIONAL adapter that produces that callback, not a core provider layer.

**NOT exercised against the live API.** No API key, and the transport is an in-memory byte stream. All SSE fixtures are hand-built from the documented formats, so they test my reading of the docs, not OpenRouter's real behaviour. Concepts only: no code from rig, genai, async-openai, swiftide or Pi was read or copied; nothing from them is a dependency.

## Run

```
cd prototypes/provider-abstraction
cargo test                                          # 14 tests, all pass
cargo check -p provider-codec --target wasm32v1-none   # passes: the codec half builds no_std + alloc
```

## Layout: two crates, split along the no-I/O line

- `provider-codec` (`#![no_std]` + `alloc`, dep: `serde_json` with `alloc` only). Everything the core could see: message/reply/error types, the `Provider` and `Decoder` traits, SSE line parser, stream `Assembler`, the OpenRouter codec, the stub codec. Pure: bytes and structs in, bytes and structs out.
- `provider-adapter` (std, dep: the codec only; `futures` is dev-only). The one place that awaits: `Transport` (host's HTTP client, one method) and `Body` (pull-based chunks; dropping it must close the connection), `complete()` (drives any `Provider`), `model_callback()` (the session-loop callback shape). No runtime types.

## The trait

```rust
pub trait Provider {
    type Decoder: Decoder;
    fn request(&self, req: &ModelRequest) -> HttpRequest;          // pure encode
    fn decoder(&self, head: &ResponseHead) -> Self::Decoder;       // sees status+headers
}
pub trait Decoder {
    fn push(&mut self, bytes: &[u8], out: &mut Vec<StreamEvent>);  // any chunking
    fn finish(&mut self, out: &mut Vec<StreamEvent>);
}
```

Events are the narrow waist: `Text`, `ToolCall{index,id?,name?,args-fragment}`, `Finish`, `Usage`, `Model`, `Error`. A shared `Assembler` folds them into a `ModelReply` (tool calls by index, args parsed once at the end, duplicate `Finish` ignored, error ends the stream and keeps the partial reply).

## Results against "validated when"

| Case | Result | Test |
|---|---|---|
| SSE comment lines (`: OPENROUTER PROCESSING`), also between events | pass | `text_stream_with_comments...` |
| Chunk boundaries splitting lines and UTF-8 (1, 2, 3, 5, 17, 64, whole) incl. a 4-byte emoji and CRLF | pass | same, and tool/error tests use the same sizes |
| Error mid-stream inside HTTP 200 (top-level `error` + `finish_reason:"error"`), partial text kept, later events ignored | pass | `mid_stream_error...` |
| Error chunk as the only event; `finish_reason:error` without error object; HTTP 200 whose body is a plain JSON error | pass | 3 tests |
| Streaming tool-call deltas: 2 parallel calls interleaved by `index`, `id`/`name` only on first fragment, args split mid-token and mid-UTF-8 | pass | `streaming_tool_call_deltas...` |
| Malformed args -> `args_error` set; empty args -> `{}` | pass | `malformed_and_empty...` |
| Truncated stream (no finish) -> `Truncated`, cut-off JSON event -> `Protocol`, partial kept | pass | `truncated_stream...` |
| HTTP 429 + `Retry-After` retryable, 402 not retryable, non-JSON 503 body | pass | `http_errors...` |
| Cancel: dropping the call future drops the body (connection) | pass | `dropping_the_call_future...` |
| Second integration (stub: non-streaming, own JSON shape, different auth header, different tool-result shape) behind the unchanged trait, same driver, same callback | pass, **zero trait changes** | `stub_integration...`, `model_callback_...` |

## Measurements

| What | Value |
|---|---|
| `Provider` | 2 methods, 1 associated type |
| `Decoder` | 2 methods (`push`, `finish`) |
| Whole abstraction | 4 methods, 1 associated type (0 if `decoder` returned `Box<dyn Decoder>`; I kept static dispatch) |
| OpenRouter integration | 222 lines (`openrouter.rs`, incl. ~25 comment/blank; request encode ~50, decode ~120) |
| Stub integration | 82 lines (`stub.rs`) |
| Shared, written once | `types.rs` 150, `assembler.rs` 98, `sse.rs` 72 (the SSE parser is shared only by SSE integrations; stub does not use it) |
| Driver + callback + transport traits | 75 lines (`provider-adapter/src/lib.rs`) |
| Tests | 275 lines, 14 tests |
| Dependencies | codec: `serde_json` (alloc) -> serde_core, itoa, memchr, zmij (5 crates incl. itself excluded: 4 external). Adapter: codec only. Dev: `futures`. |

## What leaks through the trait (the trait is clean; the types around it are not)

1. **Streaming vs buffered is invisible to the trait but not to the user.** A buffered integration (stub) emits everything in `finish()`, so the live `sink` sees nothing until the end. The trait holds; the "live deltas" promise does not.
2. **Tool-call fragmentation had to become part of the event type** (`index` + optional `id`/`name` + args string fragment). A provider with whole tool calls just sends one fragment; a provider that streams structured JSON would have to re-serialize. This is the one concept every integration is forced to speak.
3. **Errors carry the provider's vocabulary.** I normalized to 8 `ErrorKind`s, `retry_after_secs`, `retryable()`, and an opaque `metadata`. OpenRouter's finer typing (`error_type`, `provider_code`, 402 `limit_source`) survives only inside `metadata`; code that wants to branch on it must know OpenRouter.
4. **`ModelRequest` is lowest-common-denominator.** Provider-specific knobs (reasoning, `provider` routing, `models` fallbacks, `plugins`) travel as opaque `extra: Option<Value>` plus per-integration `body_defaults`. Honest but unchecked: a typo is silently ignored by OpenRouter (docs: unsupported parameters are ignored).
5. **Lossy mappings:** `ToolResult.is_error` has no slot in OpenRouter's `tool` message (the stub has one); reasoning deltas (`delta.reasoning`, `reasoning_details`) are dropped; only `choices[0]` is read (n=1); cost units are untyped `f64`.
6. **`ModelReply` is richer than session-task's** (`finish`, `usage` with cost, `model` actually served, per-call `args_error`). Session-task's `ModelReply{text, tool_calls, tokens}` would drop cost and the served model, which matter for fallbacks (`models` array) and cost roll-up.
7. **Retry policy has no home.** `ProviderError::retryable()` and `retry_after_secs` are data only; acting on them needs a sleep, i.e. a timer capability the library does not have (same finding as session-task). `complete()` does not retry.
8. **Fixtures vs reality:** both integrations are written by the same author from the same mental model, so the "second integration fits" result is weaker evidence than a real second provider (e.g. Anthropic Messages, whose streaming uses typed events with `content_block` indexes) would be. I expect it to fit through `ToolCall{index,..}` fragments, but that is a prediction.

## Where the "host-registered callback" framing changes the trait

- **The core never sees `Provider` at all.** It sees only the callback. `Provider`, `Decoder`, `Transport`, `StreamEvent` are all adapter internals. The "smallest trait" question therefore moves from the core to the adapter crate's public API. The core-facing contract is `ModelRequest`/`ModelReply`/error only.
- **Cancel is dropped from `ModelRequest`.** session-task passes a `CancelToken` into the model callback; here abort is future-drop, and dropping closes the body (tested). A transport that needs an explicit abort call (HTTP/2 RST) would want the token or a `Drop` impl; `Drop` was enough here.
- **Error type of the callback is `String`.** Flattening `ProviderError` to `String` (what `model_callback` does) loses `retryable`/`retry_after`. Either the callback's error type becomes a small structured type owned by the core (kind + retry hint + partial), or retry lives inside the adapter. The latter needs a host timer capability. Recommend the core define the small error type.
- **Streaming is out-of-band.** The callback returns one `ModelReply`; live deltas go through the `sink` closure given to `complete()`, which the core's callback shape does not have. If streaming enters the core (map says "left unspecified"), the callback either gets an event-sink argument (sync `Fn(&StreamEvent)`, like `on_event`) or returns a stream type.
- **Core-facing side can stay `no_std + alloc`.** The whole codec crate does (`cargo check -p provider-codec --target wasm32v1-none` passes), including the OpenRouter and stub codecs and the SSE parser. Only `Transport`/`Body`/`complete` need `std::future`; the HTTP client is the host's. So even the OpenRouter codec could sit inside a no-I/O crate, and only the transport is host-side. That is stronger than "optional adapter": it could ship in the gated crate.
- `Send` bounds: the callback future needs `P::Decoder: Send`, an extra bound that surfaced in the `model_callback` signature (same `Send` leak as session-task).

## Verified vs assumed

Verified from primary sources (fetched 2026-11-01):
- Streaming comment line `: OPENROUTER PROCESSING` and the Chat Completions mid-stream error shape (top-level `error` plus a choice with `finish_reason:"error"`): https://openrouter.ai/docs/api_reference/streaming
- **Streaming tool-call delta shape (was UNVERIFIED in the research):** OpenRouter's OpenAPI spec, https://openrouter.ai/openapi.json, schemas `ChatStreamDelta.tool_calls: ChatStreamToolCall[]`, where `ChatStreamToolCall` = `{index: integer (required), id?: string, type?: "function", function?: {name?: string, arguments?: string "Function arguments as JSON string"}}`; `ChatStreamChunk` has top-level `error{code,message,metadata{error_type,provider_code}}` and `usage`; `ChatFinishReasonEnum` = tool_calls, stop, length, content_filter, error, null (unknown values allowed). `ChatStreamOptions.include_usage` is deprecated: "Full usage details are always included".
- OpenRouter's tool-calling tutorial says arguments may be split across chunks and tool calls arrive "one index at a time": https://openrouter.ai/blog/tutorials/tool-calling/
- Error codes, `Retry-After`, 200-with-error behaviour: `research/openrouter-api.md` (from the docs, not re-fetched).

Assumed (not stated in any source I could read):
- That `id`/`name` appear only in the first fragment of a call and later fragments carry only `index` + an `arguments` piece. The spec makes `id`/`name` optional per fragment, so my assembler accepts either (first non-empty wins), but the real fragmenting pattern per upstream provider is unobserved.
- That fragments of different calls may interleave (handled by index; no source says whether they do).
- That a finish chunk may carry a `tool_calls` finish reason with an empty delta (handled either way).
- That the stream always ends with `data: [DONE]` (not required: a stream with a finish reason and no `[DONE]` also succeeds).
- No-arg tools sending `""` as arguments (handled as `{}`), malformed JSON arguments (handled as `args_error`): both defensive; the research lists malformed arguments as UNKNOWN.
- Lone `\r` line endings are not handled (not expected from OpenRouter).
- The stub API is invented; it verifies nothing about any real provider.

## Comparison note (not a dependency)

The ticket left open whether the adapter reuses genai or async-openai. This prototype shows the codec itself is about 220 lines and 4 small dependencies, so reuse would buy request/response types and HTTP plumbing, not the hard parts (mid-stream error, comments, delta folding), and it would pull HTTP clients into any crate meant to pass the no-I/O gate. Not measured against those crates.

## Surprises

1. **The OpenAPI spec is fetchable and is the best primary source.** `https://openrouter.ai/openapi.json` (2.7 MB) was reachable through curl even though most doc pages were not on the WebFetch allowlist; it settled the UNVERIFIED delta format (only partially: the per-fragment pattern is still unobserved).
2. **The codec fits in `no_std` + `alloc` with no extra work.** The cost is only that the HTTP transport must be a host seam. That blurs the map's "optional std-side adapter" framing: the OpenRouter codec could live in the gated core crate while only the transport stays host-side.
3. **The trait stayed tiny because errors are handled by the decoder switching modes on status.** I expected a third method for error classification; passing `ResponseHead` to `decoder()` removed it. The cost is each integration reimplements error-body parsing (about 25 lines in OpenRouter).
4. **The callback's `Result<_, String>` is the real constraint**, not the trait: it cannot carry retry hints, partial output or cost (see above).
