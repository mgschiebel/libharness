//! Library-owned request/reply/error types. Shape follows the provider-abstraction prototype's
//! `model_callback` (std version: owned, serialisable-in-spirit data, no borrowed lifetimes).
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
    Assistant { text: String, tool_calls: Vec<ToolCall> },
    ToolResult { call_id: String, name: String, content: String, is_error: bool },
}
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub params: Value,
}
/// `extra` is the opaque per-request pass-through for OpenRouter-only fields
/// (`models`, `provider`, `reasoning`, `plugins`, ...).
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
}
#[derive(Clone, Debug, PartialEq)]
pub enum FinishReason {
    Stop,
    ToolCalls,
    Length,
    ContentFilter,
    Error,
    Other(String),
}
#[derive(Clone, Debug, PartialEq)]
pub struct ModelReply {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish: FinishReason,
    pub usage: Option<Usage>,
    /// The model OpenRouter actually served (differs from the requested one under fallbacks).
    pub model: Option<String>,
    /// Fields genai's typed model cannot carry, recovered from raw frames:
    /// `cost_details`, `is_byok`, `reasoning`, `reasoning_details`.
    pub meta: Option<Value>,
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
    /// Bytes arrived but did not follow the protocol.
    Protocol,
    /// The stream ended with no terminal finish reason.
    Truncated,
    /// Connect/reset/timeout, or any genai error that has no better home.
    Transport,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ModelError {
    pub kind: ErrorKind,
    pub status: Option<u16>,
    pub code: Option<i64>,
    pub message: String,
    pub retry_after_secs: Option<u64>,
    pub metadata: Option<Value>,
    /// What had been assembled when a stream died.
    pub partial: Option<Box<ModelReply>>,
}
impl ModelError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, status: None, code: None, message: message.into(), retry_after_secs: None, metadata: None, partial: None }
    }
    pub fn retryable(&self) -> bool {
        matches!(self.kind, ErrorKind::RateLimited | ErrorKind::Upstream | ErrorKind::Transport)
    }
}
impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}
