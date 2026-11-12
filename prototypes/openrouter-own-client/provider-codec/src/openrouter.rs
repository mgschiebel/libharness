//! OpenRouter Chat Completions codec. Sources: openrouter.ai/docs/api_reference/{streaming,errors-and-debugging},
//! https://openrouter.ai/openapi.json (schemas ChatStreamChunk / ChatStreamDelta / ChatStreamToolCall).
use crate::sse::SseParser;
use crate::types::*;
use crate::{Decoder, Provider};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde_json::{json, Map, Value};

pub struct OpenRouter {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
    /// Extra headers (HTTP-Referer, X-Title) and default body fields (provider, models, plugins...).
    pub headers: Vec<(String, String)>,
    pub body_defaults: Map<String, Value>,
}
impl OpenRouter {
    pub fn new(api_key: &str, model: &str) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            base_url: "https://openrouter.ai/api/v1".into(),
            headers: Vec::new(),
            body_defaults: Map::new(),
        }
    }
}

fn message_json(m: &Message) -> Value {
    match m {
        Message::User(t) => json!({"role": "user", "content": t}),
        Message::Assistant { text, tool_calls, reasoning_details } => {
            let mut v = json!({"role": "assistant", "content": if text.is_empty() && !tool_calls.is_empty() { Value::Null } else { json!(text) }});
            if !tool_calls.is_empty() {
                v["tool_calls"] = tool_calls
                    .iter()
                    .map(|c| json!({"id": c.id, "type": "function",
                        "function": {"name": c.name, "arguments": c.args.to_string()}}))
                    .collect();
            }
            if !reasoning_details.is_empty() {
                v["reasoning_details"] = Value::Array(reasoning_details.clone());
            }
            v
        }
        // `is_error` has no slot in the OpenAI-shaped tool message: it is lost here.
        Message::ToolResult { call_id, content, .. } => json!({"role": "tool", "tool_call_id": call_id, "content": content}),
    }
}

impl Provider for OpenRouter {
    type Decoder = OrDecoder;
    fn request(&self, req: &ModelRequest) -> HttpRequest {
        let mut body = self.body_defaults.clone();
        body.insert("model".into(), json!(self.model));
        body.insert("stream".into(), json!(true));
        body.insert("messages".into(), req.messages.iter().map(message_json).collect());
        if !req.tools.is_empty() {
            // The research notes say tools must be resent on every turn: the session loop already does.
            body.insert(
                "tools".into(),
                req.tools
                    .iter()
                    .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.params}}))
                    .collect(),
            );
        }
        if let Some(Value::Object(extra)) = &req.extra {
            for (k, v) in extra {
                body.insert(k.clone(), v.clone());
            }
        }
        let mut headers = alloc::vec![
            ("Authorization".into(), alloc::format!("Bearer {}", self.api_key)),
            ("Content-Type".into(), "application/json".into()),
            ("Accept".into(), "text/event-stream".into()),
        ];
        headers.extend(self.headers.iter().cloned());
        HttpRequest { url: alloc::format!("{}/chat/completions", self.base_url), headers, body: Value::Object(body).to_string().into_bytes() }
    }
    fn decoder(&self, head: &ResponseHead) -> OrDecoder {
        if head.status >= 400 {
            OrDecoder::ErrorBody { status: head.status, retry_after: head.retry_after(), buf: Vec::new() }
        } else {
            OrDecoder::Sse { sse: SseParser::default(), seen_event: false, stopped: false }
        }
    }
}

pub enum OrDecoder {
    Sse { sse: SseParser, seen_event: bool, stopped: bool },
    ErrorBody { status: u16, retry_after: Option<u64>, buf: Vec<u8> },
}

/// `{"code":..,"message":..,"metadata":{..}}` -> ProviderError.
fn error_from_obj(e: &Value, status: Option<u16>, retry_after: Option<u64>) -> ProviderError {
    let code = e.get("code").and_then(Value::as_i64).or(status.map(i64::from));
    let mut pe = ProviderError::new(code.map(kind_from_code).unwrap_or(ErrorKind::Upstream), e.get("message").and_then(Value::as_str).unwrap_or("unknown error"));
    pe.status = status;
    pe.code = code;
    pe.retry_after_secs = retry_after;
    pe.metadata = e.get("metadata").cloned();
    pe
}
fn finish_reason(s: &str) -> FinishReason {
    match s {
        "stop" => FinishReason::Stop,
        "tool_calls" => FinishReason::ToolCalls,
        "length" => FinishReason::Length,
        "content_filter" => FinishReason::ContentFilter,
        "error" => FinishReason::Error,
        o => FinishReason::Other(o.into()),
    }
}
fn chunk_events(v: &Value, out: &mut Vec<StreamEvent>) {
    if let Some(e) = v.get("error") {
        out.push(StreamEvent::Error(error_from_obj(e, None, None)));
    }
    if let Some(m) = v.get("model").and_then(Value::as_str) {
        out.push(StreamEvent::Model(m.into()));
    }
    // n=1 only: choices[0].
    if let Some(c) = v.pointer("/choices/0") {
        if let Some(t) = c.pointer("/delta/content").and_then(Value::as_str) {
            if !t.is_empty() {
                out.push(StreamEvent::Text(t.into()));
            }
        }
        if let Some(t) = c.pointer("/delta/reasoning").and_then(Value::as_str) {
            if !t.is_empty() {
                out.push(StreamEvent::Reasoning(t.into()));
            }
        }
        if let Some(ds) = c.pointer("/delta/reasoning_details").and_then(Value::as_array) {
            out.push(StreamEvent::ReasoningDetails(ds.clone()));
        }
        if let Some(calls) = c.pointer("/delta/tool_calls").and_then(Value::as_array) {
            for tc in calls {
                out.push(StreamEvent::ToolCall {
                    index: tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize,
                    id: tc.get("id").and_then(Value::as_str).map(String::from),
                    name: tc.pointer("/function/name").and_then(Value::as_str).map(String::from),
                    args: tc.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("").into(),
                });
            }
        }
        if let Some(f) = c.get("finish_reason").and_then(Value::as_str) {
            out.push(StreamEvent::Finish(finish_reason(f)));
        }
    }
    if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
        out.push(StreamEvent::Usage(Usage {
            prompt_tokens: num(&u["prompt_tokens"]),
            completion_tokens: num(&u["completion_tokens"]),
            total_tokens: num(&u["total_tokens"]),
            cost: u.get("cost").and_then(Value::as_f64),
            cost_details: u.get("cost_details").filter(|d| d.is_object()).cloned(),
            is_byok: u.get("is_byok").and_then(Value::as_bool),
            reasoning_tokens: u.pointer("/completion_tokens_details/reasoning_tokens").and_then(Value::as_u64),
            cached_tokens: u.pointer("/prompt_tokens_details/cached_tokens").and_then(Value::as_u64),
        }));
    }
}

impl Decoder for OrDecoder {
    fn push(&mut self, bytes: &[u8], out: &mut Vec<StreamEvent>) {
        match self {
            OrDecoder::ErrorBody { buf, .. } => buf.extend_from_slice(bytes),
            OrDecoder::Sse { sse, seen_event, stopped } => {
                if *stopped {
                    return;
                }
                let mut datas = Vec::new();
                sse.push(bytes, &mut datas);
                handle_datas(datas, seen_event, stopped, out);
            }
        }
    }
    fn finish(&mut self, out: &mut Vec<StreamEvent>) {
        match self {
            OrDecoder::ErrorBody { status, retry_after, buf } => {
                let e = match serde_json::from_slice::<Value>(buf).ok().and_then(|v| v.get("error").cloned()) {
                    Some(e) => error_from_obj(&e, Some(*status), *retry_after),
                    None => {
                        let mut pe = ProviderError::new(kind_from_code(*status as i64), alloc::format!("HTTP {status} with unparseable body"));
                        pe.status = Some(*status);
                        pe.retry_after_secs = *retry_after;
                        pe
                    }
                };
                let mut e = e;
                e.status = Some(*status);
                out.push(StreamEvent::Error(e));
            }
            OrDecoder::Sse { sse, seen_event, stopped } => {
                if *stopped {
                    return;
                }
                let mut datas = Vec::new();
                sse.finish(&mut datas);
                handle_datas(datas, seen_event, stopped, out);
                // HTTP 200 whose body is a plain JSON error object (not SSE at all).
                if !*seen_event && !sse.stray.is_empty() {
                    match serde_json::from_str::<Value>(&sse.stray).ok().and_then(|v| v.get("error").cloned()) {
                        Some(e) => out.push(StreamEvent::Error(error_from_obj(&e, Some(200), None))),
                        None => out.push(StreamEvent::Error(ProviderError::new(ErrorKind::Protocol, "200 response was not an event stream"))),
                    }
                }
            }
        }
    }
}
fn handle_datas(datas: Vec<String>, seen: &mut bool, stopped: &mut bool, out: &mut Vec<StreamEvent>) {
    for d in datas {
        *seen = true;
        if d.trim() == "[DONE]" {
            *stopped = true;
            return;
        }
        match serde_json::from_str::<Value>(&d) {
            Ok(v) => {
                let before = out.len();
                chunk_events(&v, out);
                if out[before..].iter().any(|e| matches!(e, StreamEvent::Error(_))) {
                    *stopped = true; // docs: the stream ends after the error chunk
                    return;
                }
            }
            Err(e) => {
                out.push(StreamEvent::Error(ProviderError::new(ErrorKind::Protocol, alloc::format!("bad JSON in data event: {e}"))));
                *stopped = true;
                return;
            }
        }
    }
}
