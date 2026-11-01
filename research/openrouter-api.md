# Research: OpenRouter API surface

Resolves issue #3. Question: what does the OpenRouter API provide that libharness must handle? All claims come from OpenRouter's official docs (fetched 2026-11-01). Anything not confirmed in those pages is marked **UNKNOWN**.

Sources:

- [S1] API reference overview: https://openrouter.ai/docs/api_reference/overview
- [S2] Streaming: https://openrouter.ai/docs/api_reference/streaming
- [S3] Errors and debugging: https://openrouter.ai/docs/api_reference/errors-and-debugging
- [S4] Limits: https://openrouter.ai/docs/api_reference/limits
- [S5] Parameters: https://openrouter.ai/docs/api_reference/parameters
- [S6] Tool calling: https://openrouter.ai/docs/guides/features/tool-calling
- [S7] Provider routing: https://openrouter.ai/docs/guides/routing/provider-selection
- [S8] Model fallbacks: https://openrouter.ai/docs/guides/routing/model-fallbacks
- [S9] OpenAPI specs (linked from S1, not read in full): https://openrouter.ai/openapi.json and https://openrouter.ai/openapi.yaml

## 1. Endpoints

- `POST /api/v1/chat/completions` (chat completions), `POST /api/v1/responses` (Responses API), `GET /api/v1/generation?id=...` (generation stats) [S1].
- `GET https://openrouter.ai/api/v1/key` returns key limits and usage [S4].
- The docs also describe an Anthropic-style Messages endpoint [S2, S3, S8]. Its full shape is **UNKNOWN** (not read).
- Open question for the design: this document covers Chat Completions primarily. Whether libharness also targets Responses or Messages is a decision, not a fact from the docs.

## 2. Chat completions request

Source: [S1] unless noted.

- Auth: `Authorization: Bearer <key>`, `Content-Type: application/json`.
- Optional attribution headers: `HTTP-Referer`, `X-OpenRouter-Title` (`X-Title` also accepted), `X-OpenRouter-Categories`.
- Input: either `messages` or `prompt` is required. Message `role` is `user`, `assistant`, `system` or `tool`. `content` is a string, or `ContentPart[]` for user messages. Optional `name`. Tool messages require `tool_call_id`.
- `model` is optional; if omitted, the account default is used.
- Sampling: `max_tokens`, `temperature` (0 to 2), `top_p`, `top_k`, `seed`, `stop`, `stream`, `frequency_penalty`, `presence_penalty`, `repetition_penalty`, `logit_bias`, `min_p`, `top_a`, `prediction`.
- Tools: `tools`, `tool_choice`. Structured output: `response_format` (`json_object` or `json_schema`).
- Image parts: `image_url` with `url` (URL or base64) and optional `detail` (default `auto`).
- Plugins: `plugins` array (`id`, `enabled`; ids listed: `web`, `file-parser`, `response-healing`, `context-compression`).
- OpenRouter-only fields: `models`, `route` (`'fallback'`), `provider`, `user`, `debug` (`echo_upstream_body`, streaming only).
- Assistant prefill: end `messages` with an `assistant` message to continue it.
- Unsupported parameters are ignored, not rejected (for example `logit_bias` for non-OpenAI models, `top_k` for OpenAI models).
- When a sampling parameter is omitted, OpenRouter omits it upstream rather than injecting a default [S5].
- Provider-specific parameters (for example `safe_prompt` for Mistral) pass through to the provider [S5].
- Reasoning controls [S5]: `reasoning` map (enable, effort, max tokens, exclude), `reasoning_effort` (`xhigh`, `high`, `medium`, `low`, `minimal`, `none`), `include_reasoning` (deprecated alias of `reasoning.exclude`). `max_tokens` and `max_completion_tokens` also cover reasoning tokens on most providers, so a small limit can yield empty `content` with `finish_reason: "length"`.
- `verbosity` and `web_search_options` are OpenRouter-listed parameters [S5].
- Schema detail: `top_logprobs` is typed as a required number in the overview schema [S1]. Whether that is a docs typo is **UNKNOWN**.

## 3. Chat completions response

Source: [S1].

- Top level: `id`, `choices`, `created` (Unix timestamp), `model`, `object` (`chat.completion` or `chat.completion.chunk`), optional `system_fingerprint`, `usage`.
- `choices` is always an array; each choice has `delta` when streaming and `message` otherwise.
- `finish_reason` is normalized to `tool_calls`, `stop`, `length`, `content_filter`, `error`. The provider's raw value is in `native_finish_reason`.
- A choice may include an `error` object (`code`, `message`, optional `metadata`).
- `model` in the response names the model that actually served the request [S8].

## 4. Streaming (SSE)

Source: [S2] unless noted.

- Enabled with `stream: true`, works for all models [S1].
- Keep-alive comments such as `: OPENROUTER PROCESSING` are sent occasionally. Parsers must skip lines starting with `:` before JSON-decoding. [S1] also says clients should ignore SSE comment payloads.
- Final chunk before `[DONE]` carries `usage`. Unlike OpenAI's spec, this chunk has one choice with a content-free `delta` that repeats `finish_reason` and `native_finish_reason`. The terminal reason therefore appears twice; treat the usage chunk as accounting, not a second terminal event.
- Mid-stream errors: once the provider has returned headers the HTTP status is `200`, so later failures arrive as an SSE `data:` event with a top-level `error` field, a `choices` entry with `finish_reason: "error"`, after which the stream ends. The error chunk can be the only event; a `200` with an error chunk and no content is a failure.
- Cancellation: aborting the connection stops processing and billing for supported providers (OpenAI, Azure, Anthropic, DeepSeek). Not supported: AWS Bedrock, Groq, Google. Unsupported providers and non-streaming requests keep running and bill the full response.
- Streaming tool-call deltas on Chat Completions: **UNKNOWN**. The tool-calling page [S6] describes streaming only for the Responses API (`response.output_text.delta`, `response.output_item.done`, `response.completed`, `response.failed`, `response.incomplete`).

## 5. Tool / function calling

Source: [S6] unless noted.

- Flow: request with `tools`; model returns a tool call without running it; client executes; client sends a follow-up with the original input, the tool call and the result.
- Chat Completions shape: tools nested under `function` (`name`, `description`, JSON Schema `parameters`) [S1, S6]. The assistant message carries `tool_calls` (`id`, `type`, `function.name`, `function.arguments`). Results return as `role: "tool"` messages with `tool_call_id` and `content`.
- `tool_choice`: `"auto"` (default), `"none"`, forced `{ "type": "function", "function": { "name": "..." } }` on Chat Completions. [S5] also lists `required`.
- `parallel_tool_calls`: boolean, default true, only applies when tools are provided [S5, S6]. Setting false requests one call at a time [S6].
- `tools` must be sent on both the initial and the follow-up request, so the router can validate the schema [S6].
- Non-OpenAI providers get the OpenAI-shaped tool request transformed for them [S5].
- Reasoning/assistant items should be preserved across tool turns because some providers need them (stated for the Responses API streaming flow) [S6].
- OpenRouter tracks a per-provider Tool Call Error Rate used for its tool-call-aware ordering [S6]; default load balancing also scores whether tool calls are well formed [S7].
- Whether `arguments` can be malformed or invalid JSON: **UNKNOWN** (not stated in S6). A harness should still validate defensively.

## 6. Provider routing and fallbacks

Provider object (`provider` request field) [S7]:

| Field | Type | Default |
|---|---|---|
| `order` | string[] | none |
| `allow_fallbacks` | boolean | `true` |
| `require_parameters` | boolean | `false` |
| `data_collection` | `"allow"` or `"deny"` | `"allow"` |
| `zdr` | boolean | none |
| `enforce_distillable_text` | boolean | none |
| `only`, `ignore` | string[] | none |
| `quantizations` | string[] | none |
| `sort` | string or object (`by`, `partition`) | none; partition default `"model"` |
| `preferred_min_throughput`, `preferred_max_latency` | number or percentile object | none |
| `max_price` | object (`prompt`, `completion`, `request`, `image`) | none |

- Performance preferences deprioritize non-matching endpoints; `max_price` excludes them [S7].
- Default load balancing weighs price, throughput, time to first token, 429 rate, failure rate and tool-call quality, and keeps follow-ups on the same provider while the prompt cache is warm [S7]. It is bypassed by `sort`, `:nitro`/`:floor` shortcuts, `order`, or BYOK [S7].
- Model fallbacks: the `models` array is tried in priority order; any error can trigger the next model (context-length validation, moderation flags, rate limits, downtime) [S8]. You are billed at the price of the model that served the request, and the response `model` says which. A `:free` model that falls back to a paid one is billed at the paid rate [S8].
- Failover to another provider applies only before any output reaches the client [S3].
- Max length of the `models` array: **UNKNOWN** on the pages read. (The Messages endpoint `fallbacks` parameter allows at most 3 entries and cannot be combined with `models` [S8].)

## 7. Usage and cost reporting

Source: [S1] unless noted.

- `usage.prompt_tokens`, `completion_tokens`, `total_tokens`, counted with the model's native tokenizer (not normalized across models).
- `prompt_tokens_details`: `cached_tokens`, `cache_write_tokens`, `audio_tokens`, `video_tokens`.
- `completion_tokens_details`: `reasoning_tokens`, `audio_tokens`, `image_tokens`.
- Cost: `cost`, `is_byok`, `cost_details` (`upstream_inference_cost`, `upstream_inference_prompt_cost`, `upstream_inference_completions_cost`, `server_tool_cost`). `server_tool_use.web_search_requests` counts server-side searches.
- Usage is always returned for non-streaming requests; in streams it arrives in the last chunk (section 4). `GET /api/v1/generation?id=<response id>` is for later or asynchronous auditing. Its response schema is **UNKNOWN** (not read).
- Units of `cost` (credits vs USD): **UNKNOWN** in the pages read.
- Billing can occur even when no content is returned, for prompt processing [S3].

## 8. Errors

Source: [S3] unless noted.

- Body: `{ "error": { "code": number, "message": string, "metadata"?: object } }`.
- For invalid requests and exhausted credits the HTTP status equals `error.code`. Errors during generation return HTTP `200` with the error in the body or in an SSE event. Always check for `error` on a `200`.
- Codes: 400 bad request, 401 bad credentials, 402 insufficient credits, 403 forbidden (permissions, guardrail, moderation), 408 timeout, 429 rate limited, 502 model down or invalid response, 503 no provider meets routing requirements.
- `Retry-After` is sent on 429 and 503, and on 402 when `metadata.limit_source` is `openrouter_in_flight_budget`. SDKs do not auto-retry 402.
- Metadata: moderation (`reasons`, `flagged_input` truncated to 100 chars, `provider_name`, `model_slug`); budget 403 (`limit_source`, `scope`, `limit_usd`, `spend_usd`, `resets_at`, `remedy_hint`; branch on `limit_source` not the hint text); provider errors (`error_type` stable typed code, `provider_code` upstream code when available; on 500 these are omitted and `error_type` is `server`).
- Non-streaming: partial output returns `200` with `finish_reason: "error"`, content and an `error` object; no output returns `200` with only an `error` object and no `choices`.
- No-content `200`s happen on cold start, or when a reasoning model spends all of `max_tokens` (`finish_reason: "length"`, empty `content`); retrying does not help.
- 402 `limit_source` values: `openrouter_in_flight_budget`, `openrouter_key_limit`, `openrouter_credits` [S4]. `reason: weight_exceeds_budget` means retry is futile [S4].

## 9. Rate limits and credits

Source: [S4].

- Free models (id ends in `:free`): 20 requests per minute; 50 per day if under 10 credits ever purchased, 1000 per day otherwise.
- Paid models: no documented fixed request-per-minute limit on the page. Per-model/provider limits are **UNKNOWN**; upstream 429s are possible and fallback routing retries other providers.
- Other limits: account balance, optional per-key credit limit, in-flight spending budget (OpenRouter holds an estimated cost per paid request), org budgets (guardrail, workspace, key) that reset in UTC.
- 429 `error_type` is `rate_limit_exceeded`; may originate upstream (`metadata.provider_code`). Mid-stream, a rate limit arrives as an SSE error event rather than an HTTP status.
- Headers: successful responses carry no `X-RateLimit-*` headers. Platform 429s carry `X-RateLimit-Limit`, `X-RateLimit-Remaining`, `X-RateLimit-Reset`. `Retry-After` appears when providers give a hint.
- `GET /api/v1/key` reports limit, remaining, usage by period, `free_model_daily_requests`, `is_free_tier`; its `rate_limit` field is deprecated.

## 10. Where it deviates from OpenAI

All from [S1] and [S2] unless noted:

1. Schema normalized across providers; `finish_reason` normalized, raw value in `native_finish_reason`.
2. Unsupported parameters are silently ignored instead of rejected.
3. The streaming usage chunk has a non-empty `choices` with a content-free delta (OpenAI uses an empty array).
4. SSE comment lines (`: OPENROUTER PROCESSING`) must be skipped.
5. Errors can arrive as HTTP 200 (non-streaming body or SSE event) with `finish_reason: "error"`.
6. OpenRouter-only request fields: `models`, `route`, `provider`, `plugins`, `debug`, `reasoning`, `verbosity`, `web_search_options`.
7. Cost and routing metadata (`cost`, `cost_details`, `is_byok`, response `model` possibly differing from the request).
8. Tools must be resent on follow-up requests [S6].
9. Token counts use each model's native tokenizer [S1].

## 11. Items libharness must handle (derived)

These are implications of the facts above, not claims from the docs:

- SSE parser: skip `:` comment lines, tolerate the duplicated terminal finish reason, stop on `[DONE]`, detect the in-band error chunk.
- Treat HTTP 200 with an `error` field as failure in both streaming and non-streaming modes.
- Retry policy keyed on status and `Retry-After`, with no blind retry on 402 or on 403 budget errors.
- Typed model for `usage` including optional detail and cost fields.
- Passing through the `provider`, `models`, `plugins` and `reasoning` request fields, plus unknown provider-specific parameters.
- Resending `tools` on every turn; defensive argument validation.
- Cancellation by dropping the connection.

## Unknowns summary

- Chat Completions streaming tool-call delta format.
- Generation endpoint response schema; units of `cost`.
- Rate limits for paid models.
- Maximum `models` array length for Chat Completions.
- Full Messages endpoint behavior.
- Whether `top_logprobs` is truly required.
- Prompt caching request controls (`cache_control`): not researched.
- Model listing endpoint (`/models`) shape: not researched.
