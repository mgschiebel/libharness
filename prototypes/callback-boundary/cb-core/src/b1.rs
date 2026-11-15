//! B1 + R1 + K2. Closures with handles, the capability set as a value, two registration forms.
//!
//! * The callbacks get real handle types: a `CancelToken` (sync hooks), an event `Sink`, and
//!   a transport returns a `Box<dyn BodyStream>` whose `Drop` closes the connection.
//! * `Caps1` is a value: name -> callback, `O(1)` to clone (one `Arc`), built by the host and
//!   passed to each run. The library keeps nothing (`Run1` is dropped with the run).
//! * K2: `tool` (async) and `tool_blocking` (sync). A blocking tool is handed to the host's
//!   `Runner` capability; with no runner registered the registration is a typed error.
use crate::link::*;
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use call_core::*;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use serde_json::Value;
use spin::Mutex;

pub type Sink = Arc<dyn Fn(Ev) + Send + Sync>;
pub struct ModelCtx {
    pub cancel: CancelToken,
    pub sink: Sink,
}
#[derive(Clone)]
pub struct ToolCtx1 {
    pub call_id: String,
    pub cancel: CancelToken,
    pub sink: Sink,
}
pub type ModelFn1 = Arc<dyn Fn(ModelRequest, ModelCtx) -> BoxFut<Result<ModelReply, String>> + Send + Sync>;
pub type ToolFn1 = Arc<dyn Fn(ToolCtx1, Value) -> BoxFut<Result<Value, String>> + Send + Sync>;
pub type TransportFn1 =
    Arc<dyn Fn(HttpReq, CancelToken) -> BoxFut<Result<(HttpHead, Box<dyn BodyStream>), String>> + Send + Sync>;
pub type ValidateFn1 = Arc<dyn Fn(&Value) -> Result<(), String> + Send + Sync>;
/// The blocking-runner capability: "run this closure, once, on a thread you own, soon".
/// The core cannot start a thread; the host (or an opt-in std crate) can.
pub type Runner = Arc<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>;

#[derive(Clone, Debug, PartialEq)]
pub enum RegisterError {
    /// K2: a blocking callback was registered before any blocking runner.
    NoBlockingRunner { tool: String },
    Duplicate(String),
}

#[derive(Clone)]
pub(crate) struct Inner {
    pub(crate) spec: Spec,
    pub(crate) validate: ValidateFn1,
    pub(crate) model: Option<ModelFn1>,
    pub(crate) transport: Option<TransportFn1>,
    pub(crate) tools: Vec<(String, ToolFn1)>,
    pub(crate) runner: Option<Runner>,
}

/// R1: the capability set as a value. Cheap to clone; every run gets its own clone.
#[derive(Clone)]
pub struct Caps1(pub(crate) Arc<Inner>);

impl Caps1 {
    pub fn new(result_schema: Value, validate: impl Fn(&Value) -> Result<(), String> + Send + Sync + 'static) -> Self {
        Caps1(Arc::new(Inner {
            spec: Spec { tools: Vec::new(), result_schema },
            validate: Arc::new(validate),
            model: None,
            transport: None,
            tools: Vec::new(),
            runner: None,
        }))
    }
    fn me(&mut self) -> &mut Inner {
        Arc::make_mut(&mut self.0)
    }
    pub fn spec(&self) -> &Spec {
        &self.0.spec
    }
    /// How many handles to this capability value exist (the library's own runs add none after they end).
    pub fn holders(&self) -> usize {
        Arc::strong_count(&self.0)
    }
    pub fn model(mut self, f: impl Fn(ModelRequest, ModelCtx) -> BoxFut<Result<ModelReply, String>> + Send + Sync + 'static) -> Self {
        self.me().model = Some(Arc::new(f));
        self
    }
    pub fn transport(mut self, f: impl Fn(HttpReq, CancelToken) -> BoxFut<Result<(HttpHead, Box<dyn BodyStream>), String>> + Send + Sync + 'static) -> Self {
        self.me().transport = Some(Arc::new(f));
        self
    }
    pub fn blocking_runner(mut self, r: Runner) -> Self {
        self.me().runner = Some(r);
        self
    }
    pub(crate) fn add(&mut self, name: &str, desc: &str, params: Value, f: ToolFn1) -> Result<(), RegisterError> {
        let me = self.me();
        if me.tools.iter().any(|(n, _)| n == name) {
            return Err(RegisterError::Duplicate(name.to_string()));
        }
        me.spec.tools.push(ToolSpec { name: name.to_string(), description: desc.to_string(), params });
        me.tools.push((name.to_string(), f));
        Ok(())
    }
    /// Async registration: the contract is "must not block" (K1's only form).
    pub fn tool(
        mut self,
        name: &str,
        desc: &str,
        params: Value,
        f: impl Fn(ToolCtx1, Value) -> BoxFut<Result<Value, String>> + Send + Sync + 'static,
    ) -> Result<Self, RegisterError> {
        self.add(name, desc, params, Arc::new(f))?;
        Ok(self)
    }
    /// Blocking registration (K2/K3). Needs a runner registered first.
    pub fn tool_blocking(
        mut self,
        name: &str,
        desc: &str,
        params: Value,
        f: impl Fn(ToolCtx1, Value) -> Result<Value, String> + Send + Sync + 'static,
    ) -> Result<Self, RegisterError> {
        let runner = self.0.runner.clone().ok_or_else(|| RegisterError::NoBlockingRunner { tool: name.to_string() })?;
        let f = Arc::new(f);
        self.add(
            name,
            desc,
            params,
            Arc::new(move |ctx, args| {
                let shot = Shot::new();
                let tx = shot.clone();
                let f = f.clone();
                runner(Box::new(move || tx.set(f(ctx, args)))); // starts now, like an async tool
                Box::pin(shot)
            }),
        )?;
        Ok(self)
    }
}

/// A one-value channel with a waker, no_std. Completed by the runner's thread.
struct Shot<T>(Arc<Mutex<(Option<T>, Option<Waker>)>>);
impl<T> Clone for Shot<T> {
    fn clone(&self) -> Self {
        Shot(self.0.clone())
    }
}
impl<T> Shot<T> {
    fn new() -> Self {
        Shot(Arc::new(Mutex::new((None, None))))
    }
    fn set(&self, v: T) {
        let w = {
            let mut g = self.0.lock();
            g.0 = Some(v);
            g.1.take()
        };
        if let Some(w) = w {
            w.wake();
        }
    }
}
impl<T> Future for Shot<T> {
    type Output = T;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let mut g = self.0.lock();
        match g.0.take() {
            Some(v) => Poll::Ready(v),
            None => {
                g.1 = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

/// One run's view of a capability value, the token, the host's observer and inbox.
/// `src` yields the capability value to use for each call: R1 hands out its own clone; the
/// R2 variants (see `context`) hand out a snapshot or a live lookup.
pub struct Run1 {
    pub src: Arc<dyn Fn() -> Caps1 + Send + Sync>,
    pub token: CancelToken,
    pub sink: Sink,
    pub inbox: Inbox,
}
impl Run1 {
    pub fn new(caps: Caps1, token: CancelToken, sink: Sink, inbox: Inbox) -> Self {
        Run1 { src: Arc::new(move || caps.clone()), token, sink, inbox }
    }
}

impl Link for Run1 {
    fn model(&self, req: ModelRequest) -> BoxFut<Result<ModelReply, String>> {
        match &(self.src)().0.model {
            Some(m) => m(req, ModelCtx { cancel: self.token.clone(), sink: self.sink.clone() }),
            None => Box::pin(async { Err("no model registered".to_string()) }),
        }
    }
    fn tool(&self, call: ToolCall) -> BoxFut<Result<Value, String>> {
        let caps = (self.src)();
        match caps.0.tools.iter().find(|(n, _)| *n == call.name) {
            Some((_, f)) => f(ToolCtx1 { call_id: call.id, cancel: self.token.clone(), sink: self.sink.clone() }, call.args),
            None => {
                let m = alloc::format!("unknown tool {}", call.name);
                Box::pin(async move { Err(m) })
            }
        }
    }
    fn transport(&self, req: HttpReq) -> BoxFut<Result<(HttpHead, Box<dyn BodyStream>), String>> {
        match &(self.src)().0.transport {
            Some(t) => t(req, self.token.clone()),
            None => Box::pin(async { Err("no transport registered".to_string()) }),
        }
    }
    fn validate(&self, v: &Value) -> Result<(), String> {
        ((self.src)().0.validate)(v)
    }
    fn inbox(&self) -> Vec<(Inject, String)> {
        self.inbox.drain()
    }
    fn emit(&self, ev: &Ev) {
        (self.sink)(ev.clone())
    }
    fn cancel(&self) -> &CancelToken {
        &self.token
    }
}
