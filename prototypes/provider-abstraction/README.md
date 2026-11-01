# PROTOTYPE (throwaway): provider abstraction over OpenRouter

Ticket: mgschiebel/libharness#7. Branch `prototype/provider-abstraction`, never merged to main.

**Question:** does a small provider trait hold with OpenRouter first and a second, differently shaped integration behind it, and what leaks? Under session-as-task (`prototype/session-task`) the model call is a host-registered callback `Fn(ModelRequest) -> Future<Result<ModelReply, String>>`, so this is an OPTIONAL adapter that produces that callback, not a core provider layer.

**NOT exercised against the live API.** No API key, and the transport is an in-memory byte stream. All SSE fixtures are hand-built from the documented formats, so they test my reading of the docs, not OpenRouter's real behaviour. Concepts only: no code from rig, genai, async-openai, swiftide or Pi was read or copied; nothing from them is a dependency.

## Run

```
cd prototypes/provider-abstraction
cargo test                                          # 27 tests, all pass (14 OpenRouter/stub + 13 Anthropic)
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
| Dependencies | codec: `serde_json` (alloc) -> 4 external crates in total: serde_core, itoa, memchr, zmij. Adapter: codec only. Dev: `futures`. |

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

---

# Round 2 (ticket "Prototype: provider trait against the Anthropic Messages API", mgschiebel/libharness#30)

Branch `prototype/provider-abstraction-claude`, built on the branch above (which is untouched). Everything above this line is the round-1 write-up and still describes the OpenRouter work, except where the "What changed" table below says otherwise.

**Question:** does the existing `Provider`/`Decoder` trait hold against a real second integration, the Anthropic Messages API? What had to change, and what leaks?

**NOT exercised against the live API.** Nothing in this round has talked to Anthropic. Every test uses fixtures built from the event formats and error shapes printed in Anthropic's official docs, served through the in-memory transport. They test my reading of the docs, not Anthropic's real behaviour. The live smoke test exists but is PENDING (below). Concepts only: no Anthropic SDK or other project's code was read or copied; no SDK is a dependency.

## What was added

- `provider-codec/src/anthropic.rs`: the Anthropic codec (request encoder, SSE event decoder, error mapping). `no_std` + `alloc`, same single dependency as before.
- `provider-adapter/tests/contract.rs`: 13 new tests.
- `live-smoke/`: a separate crate (NOT in the workspace) with the host-side HTTP transport (`ureq`, blocking) and three `#[ignore]`d live tests. Kept outside the workspace so the HTTP client never touches the codec or adapter dependency graph.

## Run

```
cd prototypes/provider-abstraction
cargo test                                             # 27 pass, offline fixtures only
cargo check -p provider-codec --target wasm32v1-none   # passes: the Anthropic codec is also no_std + alloc
```

### Live smoke test (PENDING: not run by the author; the human runs it later)

```
cd prototypes/provider-abstraction
ANTHROPIC_API_KEY=sk-ant-... cargo test --manifest-path live-smoke/Cargo.toml -- --ignored --nocapture
# optional: ANTHROPIC_MODEL=claude-haiku-4-5-20251001 (the default; any model id from the docs works)
```

Three tests, a few hundred tokens in total: (1) streamed text with a system prompt, asserting text, finish, usage, served model; (2) a forced `tool_choice: any` tool call, then replay of the assistant turn plus a `tool_result` and a final text answer; (3) a deliberately invalid key must come back as `ErrorKind::Auth`, HTTP 401, with a `request_id`. Without the variable set they return immediately (message `SKIPPED` with `--nocapture`). The key is read from the environment only and is passed to the codec as a header value on the host side; nothing prints it. The transport itself was checked once against a throwaway local HTTP server (200 SSE and 401 JSON), so what is untested is Anthropic's real bytes, not the plumbing. **If the live run disagrees with a fixture, the fixture (my reading of the docs) is what is wrong.** Things most worth watching in its output: the number of text deltas, whether `message_delta.usage` carries `input_tokens`, the 401 body shape, `request-id` header presence.

## Results against "validated when"

The Anthropic integration slots in behind the **unchanged** `Provider` and `Decoder` traits (same 4 methods, same associated type; `Transport`, `Body`, `complete()` and `model_callback()` also unchanged). The shared *types* around the trait did change; see the next section.

| Case | Result | Test |
|---|---|---|
| Named SSE events: message_start, content_block_start/delta/stop, message_delta (stop reason + usage), message_stop, ping | pass at 7 chunk sizes incl. 1-byte, UTF-8 and emoji split across chunks | `anthropic_text_stream_...` |
| `text_delta` | pass | same |
| `tool_use` blocks, `input_json_delta` partial JSON, two parallel calls, block indexes that skip (text at 0, tools at 1 and 2), empty first delta, no-arg tool | pass | `anthropic_tool_use_blocks_...` |
| Thinking blocks: `thinking_delta`, `signature_delta`, `display: omitted` shape (empty text then one signature), `redacted_thinking`; replayed unchanged in the next request | pass | `anthropic_thinking_...`, `..._replays_thinking_...` |
| Unknown block types (server tools), unknown delta types (`citations_delta`), unknown events skipped; server-tool args do not become client tool calls | pass | `anthropic_unknown_...` |
| Every documented `stop_reason` mapped | pass | `anthropic_stop_reasons_...` |
| `error` event mid-stream (`overloaded_error`) inside HTTP 200: partial text and usage kept, later events ignored | pass | `anthropic_error_event_...` |
| Truncation: stream cut mid-text, cut after `message_delta` but before `message_stop`, cut mid-JSON | pass (Truncated, Truncated, Protocol) | `anthropic_truncated_...` |
| Non-2xx bodies for all ten documented statuses (400, 401, 402, 403, 404, 413, 429, 500, 504, 529), `retry-after`, `request_id` in body and header, spend-cap 429 recognised as not retryable, unknown error type falls back to status, non-JSON 502 | pass | `anthropic_non_2xx_error_bodies` |
| Request side: `x-api-key` (or `Authorization: Bearer`), `anthropic-version`, `content-type`, top-level `system`, required `max_tokens`, `tools` with `input_schema`, `extra` pass-through | pass | `anthropic_request_encoding_...` |
| Cancel: dropping the call future drops the body | pass | `anthropic_dropping_...` |
| Same driver and the session-loop callback shape | pass | `anthropic_behind_the_same_callback_...` |
| OpenRouter and stub re-run after the type changes | all 14 round-1 tests pass (one test updated for the new `system` message) | `cargo test` |

## What changed, exactly, and why

The trait did not change. Four shared types did; each is a finding for the map, not a failure.

| Change | Why (Anthropic requirement) | Cost to the other integrations |
|---|---|---|
| `ModelRequest.system: Option<String>` | Anthropic has a top-level `system` parameter and no system role in `messages`. Round 1 had **no way to express a system prompt at all** (the shared `Message` has no system variant); this went unnoticed because nothing needed it. | OpenRouter prepends a `role:system` message (3 lines). Stub ignores it. |
| `ReasoningBlock` enum (`Thinking{text,signature}`, `Redacted{data}`); `ModelReply.reasoning`; `Message::Assistant.reasoning` | Thinking blocks must be passed back **complete and unmodified** with the tool results of the same turn, and the signature is opaque. They are conversation history, not only output. Without a slot the session loop would drop them and tool use with thinking would fail. | OpenRouter and stub ignore the field (round 1 already dropped reasoning). Every `Message::Assistant` constructor needed `reasoning: vec![]`. |
| `StreamEvent::Reasoning{index,text,signature}` and `StreamEvent::RedactedReasoning{index,data}`; assembler folds them by index | The thinking and signature deltas must reach the assembler. | The assembler grew 25 lines; other codecs untouched. |
| (none) `max_tokens` | Required by Anthropic, not required by OpenRouter. I put it in integration config (`Anthropic::new(key, model, max_tokens)`) with a per-request override through `extra["max_tokens"]`. **Not** in `ModelRequest`. | None. But it is a decision: the required-ness shows up at construction, not in the request type. |

`ToolCall`, `StreamEvent::ToolCall{index,id?,name?,args}`, `Usage`, `FinishReason`, `ErrorKind`, `ProviderError` and the two traits needed **no change**. The fragment event fits Anthropic's tool shape: the `content_block_start` carries `id` and `name` (emitted as a first fragment with empty args), and each `input_json_delta` is a later fragment; the block `index` plays the role of OpenAI's `index` (it counts all blocks, so tool indexes skip numbers, which the assembler's ordered map handles).

## Measurements

| What | Round 1 | Now |
|---|---|---|
| `Provider` | 2 methods, 1 associated type | same |
| `Decoder` | 2 methods | same |
| Whole abstraction | 4 methods, 1 assoc. type | same (**trait size delta: 0**) |
| Shared event type | 6 variants | 8 variants (+`Reasoning`, `RedactedReasoning`) |
| Shared request/reply types | `ModelRequest` 3 fields, `ModelReply` 5, `Message::Assistant` 2 | 4, 6, 3 (+`ReasoningBlock`, 2 variants) |
| Anthropic integration | n/a | 386 lines (343 without comments and blanks): request encoder about 100, decoder about 190, errors about 50 |
| OpenRouter integration | 222 | 224 (+2 for system prompt) |
| Stub integration | 82 | 82 |
| `types.rs` / `assembler.rs` / `sse.rs` | 150 / 98 / 72 | 168 / 123 / 72 (the SSE parser needed no change: Anthropic's `event:` names are ignored and dispatch uses the `type` field inside `data`) |
| Tests | 275 lines, 14 tests | 584 lines, 27 tests |
| Codec dependencies | `serde_json` (alloc), 4 external crates | same, 4 |
| Live smoke crate | n/a | 157 lines (62 transport + 95 tests); `ureq` 2 with rustls; 87 crates in its tree, none in the workspace |
| `no_std` + `alloc` gate | passes | still passes (`cargo check -p provider-codec --target wasm32v1-none`) |

## What each integration leaks through the trait

The trait is still clean; the leaks are in the shared types and in what each codec must know.

Anthropic:
1. **Content is an ordered block list; the shared reply is flattened.** `ModelReply` holds one `text`, a tool-call list and a reasoning list. A turn like text, tool_use, text, tool_use is replayed as thinking, text, tool_use, tool_use. Anthropic accepts that, but it is not what the model produced, and interleaving is lost. Not verified whether replay order matters to the model.
2. **Message grouping is not 1:1.** All `tool_result` blocks of one assistant turn must share one user message, results first, any text after (tool-use page). The encoder is a fold over the history (about 30 lines), not a map. The shared `Message::ToolResult` as a separate message is fine, but anything that assumes one wire message per shared message is wrong.
3. **Opaque state must round-trip.** `signature` and `redacted_thinking.data` are meaningless to us and must come back byte for byte. A core that edits history (compaction, truncation) must know these blocks are not freely droppable inside a tool-use turn. The thinking page also says which prior-turn blocks the API keeps varies per model; nothing in the trait or types lets the codec say "this block is only valid for model X".
4. **Errors carry a string type.** Anthropic errors have `error.type` (a string) and optional `details.error_code`; `ProviderError.code` is a numeric slot that round 1 designed for OpenRouter. The string rides in opaque `metadata` again. Mid-stream errors have no HTTP status at all (`status: None`, `code: None`), so `retryable()` rests only on the kind.
5. **Retryability is not always the kind.** A tier spend-cap 429 has the same error type as a rate limit but retrying never works; the docs tell them apart by `details.error_code` and the absence of `retry-after`. I special-cased it to `ErrorKind::Credits`. A third provider will have its own such exceptions; `retryable()` as a function of the kind alone will not scale.
6. **`FinishReason` is lossy.** `pause_turn` (server-tool loop hit its limit; the caller must send the assistant content back to continue) lands in `Other("pause_turn")`; `stop_details` on refusals and which `stop_sequence` fired are dropped. `pause_turn` is a control-flow signal the session loop would have to know about.
7. **Usage semantics differ.** Anthropic usage is cumulative across `message_start` and `message_delta`, `message_delta` may carry only `output_tokens`, and `input_tokens` excludes cached tokens. The decoder keeps merged state and sums the three input counts; `cost` is always `None` (Anthropic returns none). Cache read/write counts are not exposed; `Usage` has no slot for them.
8. **Provider knobs are still opaque `extra`.** `thinking`, `tool_choice`, `max_tokens` override go through unchecked JSON. Version-dependent: the latest `anthropic-version` removes `temperature`, `top_p` and `top_k` (400 on every model), and thinking configuration differs per model (some models 400 on `enabled` with `budget_tokens`). Where OpenRouter ignores unknown parameters, Anthropic rejects bad ones, so `extra` mistakes surface only at runtime.
9. **Auth style is a choice.** The docs now list `Authorization: Bearer` as primary and `x-api-key` as "legacy fallback ... still supported". The codec has an `Auth` enum; default `x-api-key` as the ticket asked.

OpenRouter (unchanged from round 1, plus): reasoning deltas are still dropped, so OpenRouter-with-thinking-models cannot yet round-trip reasoning either; the new `ReasoningBlock` could carry OpenRouter's `reasoning_details` later.

Common to both: parsing of tool arguments happens once at the end (in the assembler), not at `content_block_stop`, so a UI cannot show a parsed partial call; the callback still returns `Result<_, String>` (round 1 finding stands, and now has a second example of what a `String` loses: the request id, error type, retry hint, partial output).

## Verified vs assumed

Fetched 2026-11-01 as markdown (`curl` of the `.md` form of each page on platform.claude.com; the pages are the current official docs; the older docs.anthropic.com URLs were not fetchable through the fetch tool).

Verified (read in the docs):
- Streaming page, https://platform.claude.com/docs/en/build-with-claude/streaming : named events with the matching `type` in the data; flow message_start, then content blocks (start, deltas, stop; `index` equals the position in the final `content`), then one or more `message_delta`, then `message_stop`; `ping` events anywhere; `usage` in `message_delta` is cumulative; `error` event shape `{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}` (normally HTTP 529); unknown event types must be handled gracefully; `text_delta`; `input_json_delta` with `partial_json` where the final `input` is an object and the tool block starts with `input:{}`; `thinking_delta`; `signature_delta` just before `content_block_stop`; with `display: "omitted"` an empty `thinking_delta` then a single `signature_delta`; the full text and tool-use example transcripts (used as the fixture shapes).
- Errors page, https://platform.claude.com/docs/en/api/errors : status to `type` table (400 invalid_request_error, 401 authentication_error, 402 billing_error, 403 permission_error, 404 not_found_error, 409 conflict_error, 413 request_too_large, 429 rate_limit_error, 500 api_error, 504 timeout_error, 529 overloaded_error); body shape `{"type":"error","error":{"type","message"},"request_id"}`; `request-id` response header; an error can occur after a 200 in an SSE stream; values may expand over time.
- Rate limits page, https://platform.claude.com/docs/en/api/rate-limits : `retry-after` seconds on rate-limit 429; the spend-cap 429 has no `retry-after` and `error.details.error_code` is `enforced_spend_limit_reached`.
- API overview, https://platform.claude.com/docs/en/api/overview : headers `Authorization: Bearer` (primary), `x-api-key` (legacy fallback, still supported), `anthropic-version` (required), `content-type`.
- Versions page, https://platform.claude.com/docs/en/api/versioning : `2026-10-21` is latest; `2023-06-01` introduced named SSE events and removed `data: [DONE]`; new enum values and event types may be added within a version.
- Messages reference, https://platform.claude.com/docs/en/api/messages : `max_tokens` is a required body parameter; `system` is an optional top-level string or text-block array; `tools` entries have `input_schema`; consecutive same-role turns are combined into one.
- Stop reasons page, https://platform.claude.com/docs/en/build-with-claude/handling-stop-reasons : `end_turn`, `max_tokens`, `stop_sequence`, `tool_use`, `pause_turn`, `refusal` (with `stop_details`), `model_context_window_exceeded`.
- Tool-call page, https://platform.claude.com/docs/en/agents-and-tools/tool-use/handle-tool-calls : results go in a `user` message as `tool_result` blocks with `tool_use_id`, optional `content`, optional `is_error`; `tool_result` blocks come first, text after.
- Thinking page, https://platform.claude.com/docs/en/build-with-claude/thinking : blocks carry an opaque `signature`; within a tool-use turn thinking blocks must be passed back complete and unmodified; `redacted_thinking` with opaque `data`, also passed back unchanged.
- Prompt caching page, https://platform.claude.com/docs/en/build-with-claude/prompt-caching : `input_tokens` excludes cache reads and writes; total input is the sum of the three.

Assumed (not stated in anything I read, or only seen in examples):
- Which tool-use fragments carry what beyond the one printed example: that `content_block_start` always carries `id` and `name` for `tool_use`, that `input` in the start event is always the `{}` placeholder (a non-empty one would be ignored here), and that the first delta may be an empty string (both shown in the example only).
- That `message_delta` usage may omit `input_tokens` (the examples show `output_tokens` only, one example shows `input_tokens` repeated, which the merge also handles).
- That a `text` block's start event has empty text (a non-empty one is emitted as text).
- Exact `error.type` strings other than the documented table (unknown types fall back to the HTTP status; mid-stream errors have no status, so they fall back to `Upstream`).
- That consecutive `ToolResult`s and a following `User` text can be merged into one user message (the docs require results first and text after; merging is my choice and consistent with that, and with the statement that consecutive turns are combined).
- That an assistant turn with no content blocks is invalid; the encoder emits `content: []` for it (not checked; a real session never produces one).
- That replaying text before tool_use blocks, in place of the original interleaving, is accepted.
- That a `Null`-argument call (unparseable model JSON) can be replayed as `{}`; not checked.
- That `claude-haiku-4-5-20251001` is a valid model id for the live test (it is in the streaming page's supported-models list).
- The docs' curl examples use `anthropic-version: 2023-06-01`, the version page calls `2026-10-21` latest; the codec defaults to `2023-06-01` and takes the version as a field. Which is right for libharness is untested.
- Fixtures: all are mine, written from the docs' printed examples; no captured traffic.

## Surprises

1. **Round 1 had no system prompt.** `ModelRequest` could not express one, so the shared types were incomplete in a way the OpenRouter and stub work never exposed. A second real integration found it on the first request-encoding test.
2. **Thinking blocks are history.** The reply type, the history type and the stream event type all needed a reasoning slot, because the API requires opaque signed blocks back within a tool-use turn. The trait did not move; the types around it did. For the session loop this means history must store provider-opaque blocks it cannot interpret, which touches the "session history" fog on the map.
3. **The docs moved under the ticket.** `Authorization: Bearer` is now primary and `x-api-key` is called legacy; `anthropic-version` has a newer value (`2026-10-21`) that removes sampling parameters; the official docs are at platform.claude.com (docs.anthropic.com is not on the fetch allowlist but the markdown form of the new URLs is fetchable with `curl`).
4. **The shell reached the official docs as clean markdown** (`<page URL>.md`), which was a better source than the allowlisted fetch tool, as the OpenAPI file was for OpenRouter.
5. **Retry-ability is not a function of the error kind** (spend-cap 429), which weakens round 1's `ProviderError::retryable()`.
6. Only confirmed: streaming tool-call fragments (`index`, `id?`, `name?`, `args`) fit Anthropic's block model without change; the `Decoder` could hold all Anthropic state (usage merge, block kinds, stop reason) privately; the codec stayed `no_std` + `alloc`.
