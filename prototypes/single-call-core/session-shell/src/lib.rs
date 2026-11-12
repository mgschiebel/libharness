//! PROTOTYPE (throwaway): the shell around a loop that is *registered* as a callback.
//!
//! The core owns no loop. This crate does not either: it holds the one thing that an
//! external steer/abort/observe needs while a loop owns the state (a shared inbox and a
//! token), a registration hook for the loop itself (`LoopFn`, a callback like any other:
//! state in, state out), and the typed edge (`R`, `on_result`). The loop is somebody
//! else's: the host's (`with_loop(my_loop)`) or the optional shipped one.
#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;

pub use call_core::*;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::future::Future;
use core::marker::PhantomData;
use serde::de::DeserializeOwned;
use serde::Serialize;
use spin::Mutex;

// ---- the loop is a registered callback ---------------------------------------
/// State in, state out, plus the outcome. Everything crossing it is owned data except
/// `LoopCtx`, which is the capability bundle the loop is allowed to use.
pub type LoopFn = Arc<dyn Fn(LoopCtx, State) -> BoxFut<(State, Result<serde_json::Value, SessionError>)> + Send + Sync>;
pub type EventFn = Arc<dyn Fn(&Event) + Send + Sync>;

#[derive(Default)]
struct Queues {
    steer: Vec<alloc::string::String>,
    follow: Vec<alloc::string::String>,
    closed: bool,
}
/// Steer/follow-up that arrive while the loop owns the state wait here.
#[derive(Default)]
pub struct Inbox(Mutex<Queues>);

/// What a registered loop is handed: the host's callbacks, the cancel token, the inbox.
#[derive(Clone)]
pub struct LoopCtx {
    pub cb: Callbacks,
    pub cancel: CancelToken,
    inbox: Arc<Inbox>,
    on_event: Option<EventFn>,
}
impl LoopCtx {
    /// Apply queued steer/follow-up to the state. The loop decides when (a loop that
    /// never calls this ignores steering).
    pub fn sync(&self, mut s: State) -> State {
        let mut q = self.inbox.0.lock();
        s.steer.append(&mut q.steer);
        s.follow.append(&mut q.follow);
        s
    }
    pub fn emit(&self, events: &[Event]) {
        if let Some(f) = &self.on_event {
            events.iter().for_each(|e| f(e));
        }
    }
}

// ---- the host-facing registration + handle ------------------------------------
#[derive(Clone, Debug, PartialEq)]
pub enum SessionEnd {
    Completed,
    Aborted,
    Failed,
}
#[derive(Clone)]
pub struct SessionConfig {
    pub id: SessionId,
    pub cfg: Config,
}
impl SessionConfig {
    pub fn new(id: u64) -> Self {
        Self { id: SessionId(id), cfg: Config::default() }
    }
}

type ResultFn<R> = Arc<dyn Fn(SessionId, Result<R, SessionError>) -> BoxFut<()> + Send + Sync>;

pub struct Capabilities<R> {
    cb: Callbacks,
    on_result: Option<ResultFn<R>>,
    on_event: Option<EventFn>,
    loop_fn: Option<LoopFn>,
    _r: PhantomData<fn() -> R>,
}
impl<R> Clone for Capabilities<R> {
    fn clone(&self) -> Self {
        Self { cb: self.cb.clone(), on_result: self.on_result.clone(), on_event: self.on_event.clone(), loop_fn: self.loop_fn.clone(), _r: PhantomData }
    }
}
impl<R: DeserializeOwned + Send + 'static> Capabilities<R> {
    pub fn new(result_schema: serde_json::Value) -> Self {
        Self { cb: Callbacks::typed::<R>(result_schema), on_result: None, on_event: None, loop_fn: None, _r: PhantomData }
    }
    pub fn model<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(ModelRequest, CancelToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<ModelReply, alloc::string::String>> + Send + 'static,
    {
        self.cb = self.cb.model(f);
        self
    }
    pub fn tool<F, Fut>(mut self, name: &str, description: &str, params: serde_json::Value, f: F) -> Self
    where
        F: Fn(ToolCtx, serde_json::Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<serde_json::Value, alloc::string::String>> + Send + 'static,
    {
        self.cb = self.cb.tool(name, description, params, f);
        self
    }
    pub fn tool_typed<I, O, F, Fut>(mut self, name: &str, description: &str, params: serde_json::Value, f: F) -> Self
    where
        I: DeserializeOwned + Send + 'static,
        O: Serialize + 'static,
        F: Fn(ToolCtx, I) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<O, alloc::string::String>> + Send + 'static,
    {
        self.cb = self.cb.tool_typed(name, description, params, f);
        self
    }
    pub fn on_result<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(SessionId, Result<R, SessionError>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.on_result = Some(Arc::new(move |id, r| Box::pin(f(id, r))));
        self
    }
    pub fn on_event(mut self, f: impl Fn(&Event) + Send + Sync + 'static) -> Self {
        self.on_event = Some(Arc::new(f));
        self
    }
    /// Register the loop. A callback like any other: the host's own, or the shipped one.
    pub fn with_loop(mut self, f: LoopFn) -> Self {
        self.loop_fn = Some(f);
        self
    }
}

/// Live control. Clone + Send + Sync; no runtime types.
#[derive(Clone)]
pub struct SessionHandle {
    pub id: SessionId,
    inbox: Arc<Inbox>,
    cancel: CancelToken,
}
impl SessionHandle {
    pub fn is_aborted(&self) -> bool {
        self.cancel.is_cancelled()
    }
    pub fn abort(&self) {
        self.cancel.cancel();
    }
    /// False if the session already finished.
    pub fn steer(&self, text: impl Into<alloc::string::String>) -> bool {
        let mut q = self.inbox.0.lock();
        !q.closed && {
            q.steer.push(text.into());
            true
        }
    }
    pub fn follow_up(&self, text: impl Into<alloc::string::String>) -> bool {
        let mut q = self.inbox.0.lock();
        !q.closed && {
            q.follow.push(text.into());
            true
        }
    }
}

pub struct Session;
impl Session {
    /// Returns the handle and the run as a plain future; the host decides where it runs.
    pub fn start<R: DeserializeOwned + Send + 'static>(
        caps: Capabilities<R>,
        cfg: SessionConfig,
        prompt: impl Into<alloc::string::String>,
    ) -> (SessionHandle, impl Future<Output = SessionEnd> + Send + 'static) {
        let (inbox, cancel) = (Arc::new(Inbox::default()), CancelToken::new());
        let handle = SessionHandle { id: cfg.id, inbox: inbox.clone(), cancel: cancel.clone() };
        let state = start(cfg.id, cfg.cfg, prompt);
        (handle, run(caps, cfg.id, state, inbox, cancel))
    }
    /// Run from a state that was stored earlier (possibly by another process).
    pub fn resume<R: DeserializeOwned + Send + 'static>(
        caps: Capabilities<R>,
        state: State,
    ) -> (SessionHandle, impl Future<Output = SessionEnd> + Send + 'static) {
        let (inbox, cancel, id) = (Arc::new(Inbox::default()), CancelToken::new(), state.id);
        let handle = SessionHandle { id, inbox: inbox.clone(), cancel: cancel.clone() };
        (handle, run(caps, id, state, inbox, cancel))
    }
}

async fn run<R: DeserializeOwned + Send + 'static>(
    caps: Capabilities<R>,
    id: SessionId,
    state: State,
    inbox: Arc<Inbox>,
    cancel: CancelToken,
) -> SessionEnd {
    let raw = match &caps.loop_fn {
        Some(f) => f(LoopCtx { cb: caps.cb.clone(), cancel, inbox: inbox.clone(), on_event: caps.on_event.clone() }, state).await.1,
        None => Err(SessionError::Protocol("no loop registered".into())),
    };
    inbox.0.lock().closed = true;
    let result = raw.and_then(|v| serde_json::from_value::<R>(v).map_err(|e| SessionError::Protocol(alloc::format!("typed edge: {e}"))));
    let end = match &result {
        Ok(_) => SessionEnd::Completed,
        Err(SessionError::Aborted) => SessionEnd::Aborted,
        Err(_) => SessionEnd::Failed,
    };
    if let Some(cb) = &caps.on_result {
        cb(id, result).await;
    }
    end
}
