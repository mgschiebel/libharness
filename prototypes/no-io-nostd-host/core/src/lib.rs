//! PROTOTYPE (throwaway): libharness core as no_std + alloc.
//! Callback shape copied from prototypes/session-task (branch prototype/session-task).
#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;
#[cfg(feature = "std-mutex")]
extern crate std; // PROTOTYPE(#27): only for the spin-vs-std::Mutex comparison; never in the gate

mod schema;
pub use schema::{schema_of, HasSchema};

use alloc::{boxed::Box, format, string::String, sync::Arc, vec::Vec};
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use hashbrown::HashMap;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;

/// std-free boxed future: `core::future::Future` + `alloc::boxed::Box`.
pub type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CoreError {
    #[error("model failed: {0}")]
    Model(String),
    #[error("unknown tool {0}")]
    UnknownTool(String),
    #[error("malformed final answer: {0}")]
    Malformed(String),
    #[error("no model registered")]
    NoModel,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolCall { pub id: String, pub name: String, pub args: Value }

#[derive(Clone, Debug, Default)]
pub struct ModelReply { pub text: String, pub tool_calls: Vec<ToolCall> }

#[derive(Clone, Debug, Serialize)]
pub struct ToolSpec { pub name: String, pub description: String, pub params: Value }

type ModelFn = Arc<dyn Fn(Vec<String>) -> BoxFut<Result<ModelReply, String>> + Send + Sync>;
type ToolFn = Arc<dyn Fn(Value) -> BoxFut<Result<Value, String>> + Send + Sync>;
type ClockFn = Arc<dyn Fn() -> u64 + Send + Sync>;

#[cfg(not(feature = "std-mutex"))]
type Lock<T> = spin::Mutex<T>;
#[cfg(feature = "std-mutex")]
type Lock<T> = std::sync::Mutex<T>;
#[cfg(not(feature = "std-mutex"))]
fn bump(l: &Lock<u64>) { *l.lock() += 1; }
#[cfg(not(feature = "std-mutex"))]
fn bump_read(l: &Lock<u64>) -> u64 { *l.lock() }
#[cfg(feature = "std-mutex")]
fn bump(l: &Lock<u64>) { *l.lock().unwrap() += 1; }
#[cfg(feature = "std-mutex")]
fn bump_read(l: &Lock<u64>) -> u64 { *l.lock().unwrap() }

struct ToolEntry { spec: ToolSpec, f: ToolFn }

/// Capability registry. Time is an injected capability (no `Instant` in core).
pub struct Capabilities<R> {
    model: Option<ModelFn>,
    tools: HashMap<String, Arc<ToolEntry>>,
    clock: Option<ClockFn>,
    calls: Arc<Lock<u64>>,
    result_schema: Value,
    _r: core::marker::PhantomData<fn() -> R>,
}

impl<R: DeserializeOwned + HasSchema + Send + 'static> Capabilities<R> {
    /// Result schema comes from the result type itself (typed results survive).
    pub fn new() -> Self {
        Self { model: None, tools: HashMap::new(), clock: None, calls: Arc::new(Lock::new(0)),
               result_schema: schema_of::<R>(), _r: core::marker::PhantomData }
    }
    pub fn model<F, Fut>(mut self, f: F) -> Self
    where F: Fn(Vec<String>) -> Fut + Send + Sync + 'static,
          Fut: Future<Output = Result<ModelReply, String>> + Send + 'static {
        self.model = Some(Arc::new(move |m| Box::pin(f(m))));
        self
    }
    pub fn clock(mut self, f: impl Fn() -> u64 + Send + Sync + 'static) -> Self {
        self.clock = Some(Arc::new(f));
        self
    }
    pub fn tool_typed<I, O, F, Fut>(mut self, name: &str, description: &str, f: F) -> Self
    where I: DeserializeOwned + HasSchema + Send + 'static, O: Serialize + 'static,
          F: Fn(I) -> Fut + Send + Sync + 'static,
          Fut: Future<Output = Result<O, String>> + Send + 'static {
        let f = Arc::new(f);
        let spec = ToolSpec { name: name.into(), description: description.into(), params: schema_of::<I>() };
        let call: ToolFn = Arc::new(move |v| {
            let f = f.clone();
            Box::pin(async move {
                let i: I = serde_json::from_value(v).map_err(|e| format!("bad arguments: {e}"))?;
                let o = f(i).await?;
                serde_json::to_value(o).map_err(|e| format!("{e}"))
            })
        });
        self.tools.insert(name.into(), Arc::new(ToolEntry { spec, f: call }));
        self
    }
    pub fn tool_specs(&self) -> Vec<ToolSpec> {
        let mut v: Vec<_> = self.tools.values().map(|t| t.spec.clone()).collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }
    pub fn result_schema(&self) -> &Value { &self.result_schema }
    pub fn calls_made(&self) -> u64 { bump_read(&self.calls) }
    pub fn now_ms(&self) -> u64 { self.clock.as_ref().map_or(0, |c| c()) }

    /// Minimal driver (NOT the session model, see #19): one model call, run each tool
    /// call, then decode the `final_answer` tool call into `R`.
    pub async fn run_once(&self, prompt: &str) -> Result<R, CoreError> {
        let model = self.model.as_ref().ok_or(CoreError::NoModel)?;
        let t0 = self.now_ms();
        tracing::info!(prompt_len = prompt.len(), "run_once start");
        let reply = model(alloc::vec![prompt.into()]).await.map_err(CoreError::Model)?;
        let mut answer = None;
        for c in reply.tool_calls {
            if c.name == "final_answer" { answer = Some(c.args); continue; }
            let t = self.tools.get(&c.name).ok_or_else(|| CoreError::UnknownTool(c.name.clone()))?;
            bump(&self.calls);
            let _ = (t.f)(c.args).await;
        }
        tracing::debug!(elapsed = self.now_ms() - t0, "run_once done");
        let v = answer.ok_or_else(|| CoreError::Malformed("no final_answer".into()))?;
        serde_json::from_value(v).map_err(|e| CoreError::Malformed(format!("{e}")))
    }
}

pub use futures_core::Stream;
/// futures-core Stream usage: a replayable event stream (no runtime named).
pub struct VecStream<T>(pub Vec<T>);
impl<T: Unpin> futures_core::Stream for VecStream<T> {
    type Item = T;
    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<T>> {
        if self.0.is_empty() { Poll::Ready(None) } else { Poll::Ready(Some(self.0.remove(0))) }
    }
}

pub mod demo {
    //! Representative typed result / tool args, used by probe and host tests.
    use super::*;
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    #[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
    pub struct Answer { pub total: i64, pub note: String }
    #[derive(Debug, Serialize, Deserialize)]
    #[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
    pub struct AddArgs { pub a: i64, pub b: i64 }
    #[cfg(not(feature = "schemars"))]
    impl HasSchema for Answer {
        fn schema() -> Value { serde_json::json!({"type":"object","required":["total","note"],
            "properties":{"total":{"type":"integer"},"note":{"type":"string"}}}) }
    }
    #[cfg(not(feature = "schemars"))]
    impl HasSchema for AddArgs {
        fn schema() -> Value { serde_json::json!({"type":"object","required":["a","b"],
            "properties":{"a":{"type":"integer"},"b":{"type":"integer"}}}) }
    }
    /// Scripted model + one tool; the whole public surface in one function.
    pub fn caps() -> Capabilities<Answer> {
        Capabilities::<Answer>::new()
            .clock(|| 42)
            .tool_typed("add", "add two ints", |a: AddArgs| async move { Ok(a.a + a.b) })
            .model(|_msgs| async {
                Ok(ModelReply { text: "ok".into(), tool_calls: alloc::vec![
                    ToolCall { id: "1".into(), name: "add".into(), args: serde_json::json!({"a":2,"b":3}) },
                    ToolCall { id: "2".into(), name: "final_answer".into(), args: serde_json::json!({"total":5,"note":"done"}) },
                ]})
            })
    }
    /// Poll a future to completion without a runtime (single-poll; scripted futures are ready).
    pub fn block_ready<T>(f: impl Future<Output = T>) -> Option<T> {
        let mut f = core::pin::pin!(f);
        let mut cx = Context::from_waker(core::task::Waker::noop());
        match f.as_mut().poll(&mut cx) { Poll::Ready(v) => Some(v), Poll::Pending => None }
    }
}
