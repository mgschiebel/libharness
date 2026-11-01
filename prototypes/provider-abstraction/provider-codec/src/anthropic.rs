//! Anthropic Messages API codec (PROTOTYPE). Written from the official docs only, see the README
//! "Verified vs assumed" table for the page behind each behaviour:
//!   platform.claude.com/docs/en/build-with-claude/streaming  (events, deltas, mid-stream error)
//!   platform.claude.com/docs/en/api/errors                   (status/type table, error body, request-id)
//!   platform.claude.com/docs/en/api/overview, /api/messages  (headers, body parameters)
//!   .../agents-and-tools/tool-use/handle-tool-calls          (tool_result, is_error, ordering)
//!   .../build-with-claude/thinking                           (signature, redacted_thinking, replay)
use crate::sse::SseParser;
use crate::types::*;
use crate::{Decoder, Provider};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde_json::{json, Map, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Auth {
    /// `x-api-key: <key>`. The docs now call this the "legacy fallback ... still supported".
    ApiKeyHeader,
    /// `Authorization: Bearer <key>`. The docs list this as the primary header.
    Bearer,
}
pub struct Anthropic {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
    pub auth: Auth,
    /// `anthropic-version` header (required). The docs' curl examples use 2023-06-01; the
    /// version page lists 2026-10-21 as latest (it removes temperature/top_p/top_k).
    pub version: String,
    /// `max_tokens` is REQUIRED by the API and has no slot in `ModelRequest`: it is integration
    /// config here, overridable per request through `extra`.
    pub max_tokens: u32,
    pub headers: Vec<(String, String)>,
}
impl Anthropic {
    pub fn new(api_key: &str, model: &str, max_tokens: u32) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            base_url: "https://api.anthropic.com".into(),
            auth: Auth::ApiKeyHeader,
            version: "2023-06-01".into(),
            max_tokens,
            headers: Vec::new(),
        }
    }
}

// ---- request encoding ---------------------------------------------------------------------
fn text_block(t: &str) -> Value {
    json!({"type": "text", "text": t})
}
/// The wire needs ordered content-block lists and puts all tool results of one assistant turn in ONE
/// user message (tool_result blocks first, any text after). So this is a fold over the history,
/// not a 1:1 map: consecutive `ToolResult`s (and a `User` that follows them) share a user message.
fn messages_json(msgs: &[Message]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    // the user message currently being built, as a block list (only when it holds tool results)
    let mut pending_user: Option<Vec<Value>> = None;
    fn flush(out: &mut Vec<Value>, pending: &mut Option<Vec<Value>>) {
        if let Some(blocks) = pending.take() {
            out.push(json!({"role": "user", "content": blocks}));
        }
    }
    for m in msgs {
        match m {
            Message::ToolResult { call_id, content, is_error, .. } => {
                let mut b = json!({"type": "tool_result", "tool_use_id": call_id, "content": content});
                if *is_error {
                    b["is_error"] = json!(true);
                }
                pending_user.get_or_insert_with(Vec::new).push(b);
            }
            Message::User(t) => match pending_user.as_mut() {
                Some(blocks) => blocks.push(text_block(t)), // text must come AFTER the results
                None => out.push(json!({"role": "user", "content": t})),
            },
            Message::Assistant { text, tool_calls, reasoning } => {
                flush(&mut out, &mut pending_user);
                // thinking blocks first, unchanged; text; then tool_use. Original interleaving of text
                // and tool_use blocks is NOT preserved (the shared type flattens it).
                let mut blocks: Vec<Value> = reasoning
                    .iter()
                    .map(|r| match r {
                        ReasoningBlock::Thinking { text, signature } => json!({"type": "thinking", "thinking": text, "signature": signature}),
                        ReasoningBlock::Redacted { data } => json!({"type": "redacted_thinking", "data": data}),
                    })
                    .collect();
                if !text.is_empty() {
                    blocks.push(text_block(text));
                }
                for c in tool_calls {
                    // `input` must be an object; a call whose args failed to parse is replayed as {}.
                    let input = if c.args.is_object() { c.args.clone() } else { json!({}) };
                    blocks.push(json!({"type": "tool_use", "id": c.id, "name": c.name, "input": input}));
                }
                out.push(json!({"role": "assistant", "content": blocks}));
            }
        }
    }
    flush(&mut out, &mut pending_user);
    out
}

impl Provider for Anthropic {
    type Decoder = AnthropicDecoder;
    fn request(&self, req: &ModelRequest) -> HttpRequest {
        let mut body = Map::new();
        body.insert("model".into(), json!(self.model));
        body.insert("max_tokens".into(), json!(self.max_tokens));
        body.insert("stream".into(), json!(true));
        if let Some(s) = &req.system {
            body.insert("system".into(), json!(s)); // top-level, not a message
        }
        body.insert("messages".into(), Value::Array(messages_json(&req.messages)));
        if !req.tools.is_empty() {
            body.insert(
                "tools".into(),
                req.tools.iter().map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.params})).collect(),
            );
        }
        if let Some(Value::Object(extra)) = &req.extra {
            for (k, v) in extra {
                body.insert(k.clone(), v.clone()); // thinking, tool_choice, max_tokens override...
            }
        }
        let mut headers: Vec<(String, String)> = Vec::new();
        match self.auth {
            Auth::ApiKeyHeader => headers.push(("x-api-key".into(), self.api_key.clone())),
            Auth::Bearer => headers.push(("Authorization".into(), alloc::format!("Bearer {}", self.api_key))),
        }
        headers.push(("anthropic-version".into(), self.version.clone()));
        headers.push(("content-type".into(), "application/json".into()));
        headers.extend(self.headers.iter().cloned());
        HttpRequest { url: alloc::format!("{}/v1/messages", self.base_url), headers, body: Value::Object(body).to_string().into_bytes() }
    }
    fn decoder(&self, head: &ResponseHead) -> AnthropicDecoder {
        let request_id = head.header("request-id").map(String::from);
        if head.status >= 400 {
            AnthropicDecoder::ErrorBody { status: head.status, retry_after: head.retry_after(), request_id, buf: Vec::new() }
        } else {
            AnthropicDecoder::Sse(Box::default())
        }
    }
}
use alloc::boxed::Box;

// ---- errors ---------------------------------------------------------------------------------
/// `error.type` -> kind. Documented types: invalid_request_error, authentication_error, billing_error,
/// permission_error, not_found_error, conflict_error, request_too_large, rate_limit_error, api_error,
/// timeout_error, overloaded_error. New types may appear (versioning page), so unknown -> by status.
fn error_kind(ty: &str, details_code: Option<&str>, status: Option<u16>) -> ErrorKind {
    match ty {
        // the tier spend-cap 429 has the same type but futile retries (rate-limits page)
        "rate_limit_error" if details_code == Some("enforced_spend_limit_reached") => ErrorKind::Credits,
        "rate_limit_error" => ErrorKind::RateLimited,
        "invalid_request_error" | "not_found_error" | "conflict_error" | "request_too_large" => ErrorKind::BadRequest,
        "authentication_error" | "permission_error" => ErrorKind::Auth,
        "billing_error" => ErrorKind::Credits,
        "api_error" | "timeout_error" | "overloaded_error" => ErrorKind::Upstream,
        _ => status.map(|s| kind_from_code(i64::from(s))).unwrap_or(ErrorKind::Upstream),
    }
}
/// `{"type":"error","error":{"type","message","details"?},"request_id"?}` (body) or the same object
/// as an SSE `error` event (no request_id, no HTTP status).
fn error_from_value(v: &Value, status: Option<u16>, retry_after: Option<u64>, header_request_id: Option<&str>) -> ProviderError {
    let e = v.get("error").unwrap_or(v);
    let ty = e.get("type").and_then(Value::as_str).unwrap_or("");
    let details = e.get("details").cloned();
    let dcode = details.as_ref().and_then(|d| d.get("error_code")).and_then(Value::as_str);
    let mut pe = ProviderError::new(error_kind(ty, dcode, status), e.get("message").and_then(Value::as_str).unwrap_or("unknown error"));
    pe.status = status;
    pe.code = status.map(i64::from);
    pe.retry_after_secs = retry_after;
    // No slot for a string error type in ProviderError: it rides in the opaque metadata (a leak).
    let mut md = Map::new();
    md.insert("error_type".into(), json!(ty));
    if let Some(r) = v.get("request_id").and_then(Value::as_str).or(header_request_id) {
        md.insert("request_id".into(), json!(r));
    }
    if let Some(d) = details {
        md.insert("details".into(), d);
    }
    pe.metadata = Some(Value::Object(md));
    pe
}

// ---- decoding ---------------------------------------------------------------------------------
pub enum AnthropicDecoder {
    Sse(Box<SseState>),
    ErrorBody { status: u16, retry_after: Option<u64>, request_id: Option<String>, buf: Vec<u8> },
}
#[derive(Default)]
pub struct SseState {
    sse: SseParser,
    seen_event: bool,
    stopped: bool,
    /// usage is cumulative and spread over message_start / message_delta; keep the merged view.
    usage: [u64; 4], // input, cache_creation, cache_read, output
    stop_reason: Option<FinishReason>,
    /// kinds of open blocks by index, so unknown block types (server tools etc.) are skipped.
    blocks: BTreeMap<usize, Kind>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Tool,
    Thinking,
    Other,
}

fn finish_reason(s: &str) -> FinishReason {
    match s {
        "end_turn" | "stop_sequence" => FinishReason::Stop,
        "tool_use" => FinishReason::ToolCalls,
        "max_tokens" | "model_context_window_exceeded" => FinishReason::Length,
        "refusal" => FinishReason::ContentFilter,
        o => FinishReason::Other(o.into()), // pause_turn and anything new
    }
}
impl SseState {
    fn merge_usage(&mut self, u: &Value, out: &mut Vec<StreamEvent>) {
        for (i, k) in ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens", "output_tokens"].iter().enumerate() {
            if let Some(n) = u.get(*k).and_then(Value::as_u64) {
                self.usage[i] = n; // cumulative: replace, never add
            }
        }
        // input_tokens excludes cached tokens (prompt-caching page): prompt = sum of the three.
        let prompt = self.usage[0] + self.usage[1] + self.usage[2];
        out.push(StreamEvent::Usage(Usage { prompt_tokens: prompt, completion_tokens: self.usage[3], total_tokens: prompt + self.usage[3], cost: None }));
    }
    fn event(&mut self, v: &Value, out: &mut Vec<StreamEvent>) {
        let idx = || v.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "message_start" => {
                if let Some(m) = v.pointer("/message/model").and_then(Value::as_str) {
                    out.push(StreamEvent::Model(m.into()));
                }
                if let Some(u) = v.pointer("/message/usage") {
                    self.merge_usage(u, out);
                }
            }
            "content_block_start" => {
                let b = &v["content_block"];
                let kind = match b.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" => {
                        if let Some(t) = b.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                            out.push(StreamEvent::Text(t.into()));
                        }
                        Kind::Text
                    }
                    "tool_use" => {
                        out.push(StreamEvent::ToolCall {
                            index: idx(),
                            id: b.get("id").and_then(Value::as_str).map(String::from),
                            name: b.get("name").and_then(Value::as_str).map(String::from),
                            args: String::new(), // `input: {}` in the start event is a placeholder
                        });
                        Kind::Tool
                    }
                    "thinking" => {
                        out.push(StreamEvent::Reasoning { index: idx(), text: b.get("thinking").and_then(Value::as_str).unwrap_or("").into(), signature: None });
                        Kind::Thinking
                    }
                    "redacted_thinking" => {
                        out.push(StreamEvent::RedactedReasoning { index: idx(), data: b.get("data").and_then(Value::as_str).unwrap_or("").into() });
                        Kind::Other
                    }
                    _ => Kind::Other, // server tools, fallback blocks, future types: skipped
                };
                self.blocks.insert(idx(), kind);
            }
            "content_block_delta" => {
                let d = &v["delta"];
                let kind = self.blocks.get(&idx()).copied().unwrap_or(Kind::Other);
                match d.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" if kind == Kind::Text => {
                        if let Some(t) = d.get("text").and_then(Value::as_str) {
                            out.push(StreamEvent::Text(t.into()));
                        }
                    }
                    "input_json_delta" if kind == Kind::Tool => out.push(StreamEvent::ToolCall {
                        index: idx(),
                        id: None,
                        name: None,
                        args: d.get("partial_json").and_then(Value::as_str).unwrap_or("").into(),
                    }),
                    "thinking_delta" if kind == Kind::Thinking => out.push(StreamEvent::Reasoning {
                        index: idx(),
                        text: d.get("thinking").and_then(Value::as_str).unwrap_or("").into(),
                        signature: None,
                    }),
                    "signature_delta" if kind == Kind::Thinking => out.push(StreamEvent::Reasoning {
                        index: idx(),
                        text: String::new(),
                        signature: d.get("signature").and_then(Value::as_str).map(String::from),
                    }),
                    _ => {} // citations_delta and future delta types: dropped (lossy)
                }
            }
            "message_delta" => {
                if let Some(r) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop_reason = Some(finish_reason(r));
                }
                if let Some(u) = v.get("usage") {
                    self.merge_usage(u, out);
                }
            }
            // Finish is emitted here, not at message_delta: a stream that dies between the two is
            // then a Truncated error rather than a clean end.
            "message_stop" => {
                out.push(StreamEvent::Finish(self.stop_reason.take().unwrap_or(FinishReason::Other("missing_stop_reason".into()))));
                self.stopped = true;
            }
            "error" => {
                out.push(StreamEvent::Error(error_from_value(v, None, None, None)));
                self.stopped = true;
            }
            // "ping", "content_block_stop" and unknown future events need no action.
            _ => {}
        }
    }
    fn datas(&mut self, datas: Vec<String>, out: &mut Vec<StreamEvent>) {
        for d in datas {
            if self.stopped {
                return;
            }
            self.seen_event = true;
            match serde_json::from_str::<Value>(&d) {
                Ok(v) => self.event(&v, out),
                Err(e) => {
                    out.push(StreamEvent::Error(ProviderError::new(ErrorKind::Protocol, alloc::format!("bad JSON in data event: {e}"))));
                    self.stopped = true;
                }
            }
        }
    }
}

impl Decoder for AnthropicDecoder {
    fn push(&mut self, bytes: &[u8], out: &mut Vec<StreamEvent>) {
        match self {
            AnthropicDecoder::ErrorBody { buf, .. } => buf.extend_from_slice(bytes),
            AnthropicDecoder::Sse(s) => {
                if s.stopped {
                    return;
                }
                let mut datas = Vec::new();
                s.sse.push(bytes, &mut datas);
                s.datas(datas, out);
            }
        }
    }
    fn finish(&mut self, out: &mut Vec<StreamEvent>) {
        match self {
            AnthropicDecoder::ErrorBody { status, retry_after, request_id, buf } => {
                let e = match serde_json::from_slice::<Value>(buf) {
                    Ok(v) if v.get("error").is_some() => error_from_value(&v, Some(*status), *retry_after, request_id.as_deref()),
                    _ => {
                        let mut pe = ProviderError::new(kind_from_code(i64::from(*status)), alloc::format!("HTTP {status} with unparseable body"));
                        pe.status = Some(*status);
                        pe.code = Some(i64::from(*status));
                        pe.retry_after_secs = *retry_after;
                        pe
                    }
                };
                out.push(StreamEvent::Error(e));
            }
            AnthropicDecoder::Sse(s) => {
                if s.stopped {
                    return;
                }
                let mut datas = Vec::new();
                s.sse.finish(&mut datas);
                s.datas(datas, out);
                if !s.seen_event && !s.sse.stray.is_empty() {
                    match serde_json::from_str::<Value>(&s.sse.stray) {
                        Ok(v) if v.get("error").is_some() => out.push(StreamEvent::Error(error_from_value(&v, Some(200), None, None))),
                        _ => out.push(StreamEvent::Error(ProviderError::new(ErrorKind::Protocol, "200 response was not an event stream"))),
                    }
                }
            }
        }
    }
}
