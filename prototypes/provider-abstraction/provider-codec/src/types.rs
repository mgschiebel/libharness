use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use serde_json::Value;

// ---- what the session loop sees (shape mirrors prototype/session-task) --------------------
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
    /// NEW vs session-task: set when the model's arguments string was not valid JSON.
    pub args_error: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    User(String),
    /// `reasoning` is NEW (Anthropic finding): opaque blocks that must be replayed unchanged.
    Assistant { text: String, tool_calls: Vec<ToolCall>, reasoning: Vec<ReasoningBlock> },
    ToolResult { call_id: String, name: String, content: String, is_error: bool },
}
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub params: Value,
}
/// A model "thinking" block. Anthropic requires these to be passed back unchanged (signature
/// intact) with the tool results of the same turn, so they are history, not just output.
#[derive(Clone, Debug, PartialEq)]
pub enum ReasoningBlock {
    Thinking { text: String, signature: String },
    /// Encrypted, no readable text (`redacted_thinking`).
    Redacted { data: String },
}
/// NEW vs session-task: `extra` is an opaque per-request pass-through (reasoning, provider
/// routing...). session-task's `ModelRequest.cancel` is absent: cancel = drop the future.
/// NEW in the Anthropic round: `system` (it has a top-level slot there; before, the shared types
/// had no way to express a system prompt at all).
#[derive(Clone, Debug, Default)]
pub struct ModelRequest {
    pub system: Option<String>,
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
/// session-task's `ModelReply` has `text`, `tool_calls`, `tokens` only; the rest is new.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelReply {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    /// NEW (Anthropic round): thinking blocks, in block order. Must be replayed in history.
    pub reasoning: Vec<ReasoningBlock>,
    pub finish: FinishReason,
    pub usage: Option<Usage>,
    pub model: Option<String>,
}
impl ModelReply {
    pub fn tokens(&self) -> u64 {
        self.usage.as_ref().map(|u| u.total_tokens).unwrap_or(0)
    }
}

// ---- errors ---------------------------------------------------------------------------
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
    /// The transport failed (connect, reset).
    Transport,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderError {
    pub kind: ErrorKind,
    pub status: Option<u16>,
    pub code: Option<i64>,
    pub message: String,
    pub retry_after_secs: Option<u64>,
    pub metadata: Option<Value>,
    /// What had been assembled when a stream died (text, completed tool calls).
    pub partial: Option<Box<ModelReply>>,
}
impl ProviderError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, status: None, code: None, message: message.into(), retry_after_secs: None, metadata: None, partial: None }
    }
    /// Retry only makes sense for these; 402 / 400 / 401 are futile.
    pub fn retryable(&self) -> bool {
        matches!(self.kind, ErrorKind::RateLimited | ErrorKind::Upstream | ErrorKind::Transport)
    }
}
impl core::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}
pub fn kind_from_code(code: i64) -> ErrorKind {
    match code {
        400 | 413 | 422 => ErrorKind::BadRequest,
        401 | 403 => ErrorKind::Auth,
        402 => ErrorKind::Credits,
        429 => ErrorKind::RateLimited,
        _ => ErrorKind::Upstream,
    }
}

// ---- normalized stream events (the narrow waist between codecs and the assembler) -----------
#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
    Text(String),
    /// A fragment of tool call `index`. id/name normally only in the first fragment.
    ToolCall { index: usize, id: Option<String>, name: Option<String>, args: String },
    /// NEW (Anthropic round): a fragment of thinking block `index` (text and/or its signature).
    Reasoning { index: usize, text: String, signature: Option<String> },
    /// NEW (Anthropic round): a whole `redacted_thinking` block.
    RedactedReasoning { index: usize, data: String },
    Finish(FinishReason),
    Usage(Usage),
    Model(String),
    Error(ProviderError),
}

// ---- wire descriptions (data only, no I/O) -----------------------------------------------
#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}
#[derive(Clone, Debug)]
pub struct ResponseHead {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}
impl ResponseHead {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    pub fn retry_after(&self) -> Option<u64> {
        self.header("retry-after").and_then(|v| v.trim().parse().ok())
    }
}
pub(crate) fn num(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}
