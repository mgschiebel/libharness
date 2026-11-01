//! PROTOTYPE (throwaway): session-as-task whose engine is the sans-IO core (libharness#26).
//! No I/O-capable dependency, no runtime types, never spawns.
mod cancel;
mod engine;
mod session;

pub use cancel::{CancelToken, Cancelled};
pub use engine::{Action, Engine, FINAL_ANSWER};
pub use session::{Session, SessionHandle};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub u64);

// ---- messages ---------------------------------------------------------------
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
#[derive(Clone, Debug, Default)]
pub struct ModelReply {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub tokens: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub params: Value,
}
pub struct ModelRequest {
    pub session: SessionId,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub cancel: CancelToken,
}
/// What a tool/spawn callback is handed besides its input.
#[derive(Clone)]
pub struct ToolCtx {
    pub session: SessionId,
    pub call_id: String,
    pub cancel: CancelToken,
}

// ---- outcomes ---------------------------------------------------------------
#[derive(Clone, Debug, PartialEq)]
pub enum SessionError {
    Aborted,
    ModelFailed(String),
    /// The model's final_answer never matched the host's type.
    Malformed { attempts: u32, last_error: String, raw: Value },
    /// The model stopped without ever calling final_answer.
    NoAnswer,
    MaxTurns,
}
#[derive(Clone, Debug, PartialEq)]
pub enum SessionEnd {
    Completed,
    Aborted,
    Failed,
}
#[derive(Clone, Debug)]
pub enum Event {
    TurnStart { session: SessionId, turn: u32 },
    Tokens { session: SessionId, tokens: u64 },
    ToolStart { session: SessionId, name: String },
    ToolEnd { session: SessionId, name: String, is_error: bool },
    Injected { session: SessionId, kind: &'static str },
    ResultRejected { session: SessionId, error: String },
}

#[derive(Clone)]
pub struct SessionConfig {
    pub id: SessionId,
    pub max_turns: u32,
    /// How many rejected final_answers are fed back to the model before giving up.
    pub max_result_retries: u32,
    pub max_nudges: u32,
}
impl SessionConfig {
    pub fn new(id: u64) -> Self {
        Self { id: SessionId(id), max_turns: 20, max_result_retries: 2, max_nudges: 1 }
    }
}

// ---- capability registration ------------------------------------------------
pub(crate) type ModelFn = Arc<dyn Fn(ModelRequest) -> BoxFut<Result<ModelReply, String>> + Send + Sync>;
pub(crate) type ToolFn = Arc<dyn Fn(ToolCtx, Value) -> BoxFut<Result<Value, String>> + Send + Sync>;
pub(crate) type ResultFn<R> = Arc<dyn Fn(SessionId, Result<R, SessionError>) -> BoxFut<()> + Send + Sync>;
pub(crate) type EventFn = Arc<dyn Fn(&Event) + Send + Sync>;
pub(crate) struct ToolEntry {
    pub spec: ToolSpec,
    pub f: ToolFn,
}

/// Registered up front, cloned cheaply, reusable across sessions. `R` is the type the
/// host expects back from the session.
pub struct Capabilities<R> {
    pub(crate) model: Option<ModelFn>,
    pub(crate) tools: Vec<Arc<ToolEntry>>,
    pub(crate) on_result: Option<ResultFn<R>>,
    pub(crate) on_event: Option<EventFn>,
    pub(crate) result_schema: Value,
}
impl<R> Clone for Capabilities<R> {
    fn clone(&self) -> Self {
        Self {
            model: self.model.clone(),
            tools: self.tools.clone(),
            on_result: self.on_result.clone(),
            on_event: self.on_event.clone(),
            result_schema: self.result_schema.clone(),
        }
    }
}
impl<R: DeserializeOwned + Send + 'static> Capabilities<R> {
    pub fn new(result_schema: Value) -> Self {
        Self { model: None, tools: vec![], on_result: None, on_event: None, result_schema }
    }
    /// The model call is just another capability.
    pub fn model<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(ModelRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<ModelReply, String>> + Send + 'static,
    {
        self.model = Some(Arc::new(move |r| Box::pin(f(r))));
        self
    }
    /// Untyped tool: JSON in, JSON out.
    pub fn tool<F, Fut>(mut self, name: &str, description: &str, params: Value, f: F) -> Self
    where
        F: Fn(ToolCtx, Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value, String>> + Send + 'static,
    {
        let spec = ToolSpec { name: name.into(), description: description.into(), params };
        self.tools.push(Arc::new(ToolEntry { spec, f: Arc::new(move |c, v| Box::pin(f(c, v))) }));
        self
    }
    /// Typed tool: the host's closure sees `I` and returns `O`; JSON stays inside the library.
    pub fn tool_typed<I, O, F, Fut>(self, name: &str, description: &str, params: Value, f: F) -> Self
    where
        I: DeserializeOwned + Send + 'static,
        O: Serialize + 'static,
        F: Fn(ToolCtx, I) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<O, String>> + Send + 'static,
    {
        let f = Arc::new(f);
        self.tool(name, description, params, move |ctx, v| {
            let f = f.clone();
            async move {
                let input: I = serde_json::from_value(v).map_err(|e| format!("bad arguments: {e}"))?;
                let out = f(ctx, input).await?;
                serde_json::to_value(out).map_err(|e| e.to_string())
            }
        })
    }
    /// The single terminal notification: called exactly once per session whose future
    /// is polled to completion, with the typed value or why there is none.
    pub fn on_result<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(SessionId, Result<R, SessionError>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.on_result = Some(Arc::new(move |id, r| Box::pin(f(id, r))));
        self
    }
    /// Synchronous and non-blocking by contract (observability only).
    pub fn on_event(mut self, f: impl Fn(&Event) + Send + Sync + 'static) -> Self {
        self.on_event = Some(Arc::new(f));
        self
    }
}
