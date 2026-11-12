//! Library-owned types (shape copied from our provider-abstraction prototype). Not part of the
//! adapter glue count: in the real library these live in the core.
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
    /// Set when the model's arguments string was not valid JSON.
    pub args_error: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    User(String),
    Assistant {
        text: String,
        tool_calls: Vec<ToolCall>,
    },
    ToolResult {
        call_id: String,
        name: String,
        content: String,
        is_error: bool,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub params: Value,
}
/// `extra` is an opaque per-request pass-through: `models`, `provider`, `reasoning`, `plugins`.
#[derive(Clone, Debug, Default)]
pub struct ModelRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub extra: Option<Value>,
}
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cost: Option<f64>,
    pub is_byok: Option<bool>,
    /// `cost_details` as the crate typed it, re-serialised.
    pub cost_details: Option<Value>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum FinishReason {
    Stop,
    ToolCalls,
    Length,
    ContentFilter,
    Other(String),
}
#[derive(Clone, Debug, PartialEq)]
pub struct ModelReply {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish: FinishReason,
    pub usage: Option<Usage>,
    /// Model that actually served the request (the last chunk's `model`).
    pub model: Option<String>,
}
impl ModelReply {
    pub fn tokens(&self) -> u64 {
        self.usage.as_ref().map(|u| u.total_tokens).unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    BadRequest,
    Auth,
    Credits,
    RateLimited,
    Upstream,
    /// Bytes arrived but did not follow the protocol (or the crate could not decode them).
    Protocol,
    /// The stream ended with no finish reason.
    Truncated,
    Transport,
    /// The adapter dropped or could not build something (config, request validation).
    Config,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ModelError {
    pub kind: ErrorKind,
    pub status: Option<u16>,
    pub code: Option<i64>,
    pub message: String,
    pub retry_after_secs: Option<u64>,
    pub metadata: Option<Value>,
    pub partial: Option<Box<ModelReply>>,
}
impl ModelError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            status: None,
            code: None,
            message: message.into(),
            retry_after_secs: None,
            metadata: None,
            partial: None,
        }
    }
    pub fn retryable(&self) -> bool {
        matches!(
            self.kind,
            ErrorKind::RateLimited | ErrorKind::Upstream | ErrorKind::Transport
        )
    }
}
impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}
