//! PROTOTYPE (throwaway): a core whose one operation is a single model call.
//!
//! State goes in as owned serialisable data, comes back out, and nothing is kept between
//! calls. The model and every tool are host-registered callbacks. `no_std` + `alloc`.
//!
//! Layers, lowest first:
//!   `step`     pure transitions `(State, input) -> Out`. No async, no callbacks, no clock.
//!   `cancel`   the cancel token every callback is handed (sync hooks).
//!   `registry` host-registered callbacks (`Callbacks`), boxed `Send` futures.
//!   `aio`      optional async helpers over the registry: `abortable` (the extra-poll
//!              contract), `join_all`, `call_model`, `call_tools`.
#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;

mod aio;
mod cancel;
mod registry;
mod step;

pub use aio::{abortable, join_all};
pub use cancel::{CancelToken, Cancelled};
pub use registry::{BoxFut, Callbacks, ModelFn, ToolFn, ValidateFn};
pub use step::{abort, absorb, begin, follow_up, resume, start, steer, tools_done};

use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const FINAL_ANSWER: &str = "final_answer";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub u64);

// ---- messages and call shapes (all owned, all serialisable) -----------------
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Message {
    User(String),
    Assistant { text: String, tool_calls: Vec<ToolCall> },
    ToolResult { call_id: String, name: String, content: String, is_error: bool },
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ModelReply {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub tokens: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub params: Value,
}
/// What the model callback receives: data only. The cancel token is a second argument.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelRequest {
    pub session: SessionId,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
}
/// What a tool callback is handed besides its input (the token is not data).
#[derive(Clone)]
pub struct ToolCtx {
    pub session: SessionId,
    pub call_id: String,
    pub cancel: CancelToken,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolDone {
    pub call_id: String,
    pub result: Result<Value, String>,
}

// ---- outcomes and events -----------------------------------------------------
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SessionError {
    Aborted,
    ModelFailed(String),
    /// The model's final_answer never matched the host's type.
    Malformed { attempts: u32, last_error: String, raw: Value },
    /// The model stopped without ever calling final_answer.
    NoAnswer,
    MaxTurns,
    /// An input that does not fit the state's phase (loop-owner bug), reported, not panicked.
    Protocol(String),
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Inject {
    Steer,
    FollowUp,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    TurnStart { session: SessionId, turn: u32 },
    Tokens { session: SessionId, tokens: u64 },
    ToolStart { session: SessionId, name: String },
    ToolEnd { session: SessionId, name: String, is_error: bool },
    Injected { session: SessionId, kind: Inject },
    ResultRejected { session: SessionId, error: String },
}

// ---- the state that crosses the boundary on every call -----------------------
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub max_turns: u32,
    /// How many rejected final_answers are fed back to the model before giving up.
    pub max_result_retries: u32,
    pub max_nudges: u32,
}
impl Default for Config {
    fn default() -> Self {
        Self { max_turns: 20, max_result_retries: 2, max_nudges: 1 }
    }
}
/// Read-only facts about the registered capabilities that the pure steps need. Data,
/// not state: it does not change between calls.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Spec {
    pub tools: Vec<ToolSpec>,
    pub result_schema: Value,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Phase {
    Ready,
    AwaitingModel,
    AwaitingTools,
    Done,
}
/// One tool call of the current batch, with its outcome once known.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pending {
    pub call: ToolCall,
    pub outcome: Option<(String, bool)>,
}
/// Everything the core knows about one conversation. Owned by whoever runs the loop.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    pub id: SessionId,
    pub cfg: Config,
    pub phase: Phase,
    pub messages: Vec<Message>,
    pub steer: Vec<String>,
    pub follow: Vec<String>,
    pub pending: Vec<Pending>,
    pub answer: Option<Value>,
    pub turn: u32,
    pub rejected: u32,
    pub nudges: u32,
}

/// What the loop owner has to do next.
#[derive(Clone, Debug)]
pub enum Step {
    /// Make this model call, then pass the reply to `absorb`.
    Call(ModelRequest),
    /// Run these tools (any order, any concurrency), then pass all results to `tools_done`.
    Tools(Vec<ToolCall>),
    /// Terminal. The value is the raw accepted `final_answer`; the edge types it.
    End(Result<Value, SessionError>),
}
/// A transition's result: the new state, the next step, events for observability.
#[derive(Clone, Debug)]
pub struct Out {
    pub state: State,
    pub step: Step,
    pub events: Vec<Event>,
}
