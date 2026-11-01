//! Second integration (STUB): an invented, non-streaming, non-OpenAI-shaped JSON API. It exists
//! only to prove a different wire shape slots behind the same trait.
//!   POST {base}/generate   X-Api-Key: ..
//!   {"turns":[{"who":"user|bot|tool","text":..,"calls":[{"ref","fn","input"}],"ref":..}],"functions":[{"name","about","schema"}]}
//!   -> {"reply":{"text":..,"requests":[{"ref","fn","input"}]},"stop":"end|call|limit","used":{"in":..,"out":..}}
//!   errors: any non-2xx with {"problem": "..."}
use crate::types::*;
use crate::{Decoder, Provider};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde_json::{json, Value};

pub struct StubApi {
    pub api_key: String,
    pub base_url: String,
}
impl Provider for StubApi {
    type Decoder = StubDecoder;
    fn request(&self, req: &ModelRequest) -> HttpRequest {
        let turns: Vec<Value> = req
            .messages
            .iter()
            .map(|m| match m {
                Message::User(t) => json!({"who": "user", "text": t}),
                Message::Assistant { text, tool_calls, .. } => json!({"who": "bot", "text": text,
                    "calls": tool_calls.iter().map(|c| json!({"ref": c.id, "fn": c.name, "input": c.args})).collect::<Vec<_>>()}),
                Message::ToolResult { call_id, content, is_error, .. } => json!({"who": "tool", "ref": call_id, "text": content, "failed": is_error}),
            })
            .collect();
        let functions: Vec<Value> = req.tools.iter().map(|t| json!({"name": t.name, "about": t.description, "schema": t.params})).collect();
        HttpRequest {
            url: alloc::format!("{}/generate", self.base_url),
            headers: alloc::vec![("X-Api-Key".into(), self.api_key.clone())],
            body: json!({"turns": turns, "functions": functions}).to_string().into_bytes(),
        }
    }
    fn decoder(&self, head: &ResponseHead) -> StubDecoder {
        StubDecoder { status: head.status, buf: Vec::new() }
    }
}
pub struct StubDecoder {
    status: u16,
    buf: Vec<u8>,
}
impl Decoder for StubDecoder {
    fn push(&mut self, bytes: &[u8], _out: &mut Vec<StreamEvent>) {
        self.buf.extend_from_slice(bytes); // buffered: nothing is emitted until finish()
    }
    fn finish(&mut self, out: &mut Vec<StreamEvent>) {
        let v: Value = match serde_json::from_slice(&self.buf) {
            Ok(v) => v,
            Err(e) => return out.push(StreamEvent::Error(ProviderError::new(ErrorKind::Protocol, alloc::format!("bad JSON: {e}")))),
        };
        if self.status >= 400 {
            let msg = v["problem"].as_str().unwrap_or("error");
            let mut e = ProviderError::new(kind_from_code(self.status as i64), msg);
            e.status = Some(self.status);
            return out.push(StreamEvent::Error(e));
        }
        let r = &v["reply"];
        if let Some(t) = r["text"].as_str().filter(|t| !t.is_empty()) {
            out.push(StreamEvent::Text(t.into()));
        }
        for (i, c) in r["requests"].as_array().into_iter().flatten().enumerate() {
            out.push(StreamEvent::ToolCall {
                index: i,
                id: c["ref"].as_str().map(String::from),
                name: c["fn"].as_str().map(String::from),
                args: c["input"].to_string(), // whole call in one fragment
            });
        }
        out.push(StreamEvent::Finish(match v["stop"].as_str() {
            Some("end") => FinishReason::Stop,
            Some("call") => FinishReason::ToolCalls,
            Some("limit") => FinishReason::Length,
            Some(o) => FinishReason::Other(o.into()),
            None => FinishReason::Stop,
        }));
        let (i, o) = (num(&v["used"]["in"]), num(&v["used"]["out"]));
        out.push(StreamEvent::Usage(Usage { prompt_tokens: i, completion_tokens: o, total_tokens: i + o, cost: None }));
    }
}
