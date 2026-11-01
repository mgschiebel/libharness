use crate::*;
use serde_json::json;
use std::time::Instant;
use std::collections::VecDeque;
use std::future::poll_fn;
use std::pin::pin;
use std::sync::Mutex;
use std::task::Poll;

#[derive(Default)]
struct Queues {
    steer: VecDeque<String>,
    follow: VecDeque<String>,
    closed: bool,
}
struct Shared {
    q: Mutex<Queues>,
    cancel: CancelToken,
}

/// Live control. Clone + Send + Sync; holds no runtime types.
#[derive(Clone)]
pub struct SessionHandle {
    pub id: SessionId,
    shared: Arc<Shared>,
}
impl SessionHandle {
    pub fn is_aborted(&self) -> bool {
        self.shared.cancel.is_cancelled()
    }
    pub fn abort(&self) {
        self.shared.cancel.cancel();
    }
    /// Delivered at the next turn boundary. False if the session already finished.
    pub fn steer(&self, text: impl Into<String>) -> bool {
        let mut q = self.shared.q.lock().unwrap();
        !q.closed && { q.steer.push_back(text.into()); true }
    }
    /// Delivered only when the session would otherwise end.
    pub fn follow_up(&self, text: impl Into<String>) -> bool {
        let mut q = self.shared.q.lock().unwrap();
        !q.closed && { q.follow.push_back(text.into()); true }
    }
}

pub struct Session;
impl Session {
    /// Returns the handle and the session as a plain future. The *host* decides where
    /// it runs (spawn it, or await it inline). The library never spawns.
    pub fn start<R: serde::de::DeserializeOwned + Send + 'static>(
        caps: Capabilities<R>,
        cfg: SessionConfig,
        prompt: impl Into<String>,
    ) -> (SessionHandle, impl Future<Output = SessionEnd> + Send + 'static) {
        let shared = Arc::new(Shared { q: Mutex::new(Queues::default()), cancel: CancelToken::new() });
        let handle = SessionHandle { id: cfg.id, shared: shared.clone() };
        (handle, run(caps, cfg, prompt.into(), shared))
    }
}

async fn run<R: serde::de::DeserializeOwned + Send + 'static>(
    caps: Capabilities<R>,
    cfg: SessionConfig,
    prompt: String,
    shared: Arc<Shared>,
) -> SessionEnd {
    let result = drive(&caps, &cfg, prompt, &shared).await;
    shared.q.lock().unwrap().closed = true;
    let end = match &result {
        Ok(_) => SessionEnd::Completed,
        Err(SessionError::Aborted) => SessionEnd::Aborted,
        Err(_) => SessionEnd::Failed,
    };
    if let Some(cb) = &caps.on_result {
        cb(cfg.id, result).await;
    }
    end
}

/// Poll `f`; if the token fires first, drop `f` and return None. Contract: `f` is polled
/// once more after cancel() (hooks have already run synchronously), so a callback that
/// awaits a child sees the child's abort before being dropped. Dropping is the last resort.
async fn abortable<F: Future>(tok: &CancelToken, f: F) -> Option<F::Output> {
    let mut f = pin!(f);
    let mut c = pin!(tok.cancelled());
    poll_fn(|cx| {
        if let Poll::Ready(o) = f.as_mut().poll(cx) {
            return Poll::Ready(Some(o)); // checked before the token: results win races
        }
        match c.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

/// What an in-flight callback future resolves to.
enum Done {
    Model(u64, Result<ModelReply, String>),
    Tool(String, Result<Value, String>),
}

/// Poll every in-flight callback once; resolve with all that are ready (at least one).
async fn next_done(set: &mut Vec<BoxFut<Done>>) -> Vec<Done> {
    poll_fn(|cx| {
        let (mut ready, mut i) = (vec![], 0);
        while i < set.len() {
            match set[i].as_mut().poll(cx) {
                Poll::Ready(d) => {
                    ready.push(d);
                    drop(set.swap_remove(i));
                }
                Poll::Pending => i += 1,
            }
        }
        if ready.is_empty() { Poll::Pending } else { Poll::Ready(ready) }
    })
    .await
}

/// The translation layer: run the engine's actions as registered callbacks, feed the
/// results back as inputs. No policy lives here.
async fn drive<R: serde::de::DeserializeOwned + Send + 'static>(
    caps: &Capabilities<R>,
    cfg: &SessionConfig,
    prompt: String,
    shared: &Shared,
) -> Result<R, SessionError> {
    let emit = |e: Event| {
        if let Some(f) = &caps.on_event {
            f(&e)
        }
    };
    let model = caps.model.clone().ok_or_else(|| SessionError::ModelFailed("no model registered".into()))?;
    let mut specs: Vec<ToolSpec> = caps.tools.iter().map(|t| t.spec.clone()).collect();
    specs.push(ToolSpec {
        name: FINAL_ANSWER.into(),
        description: "Call exactly when done, with the result in the requested shape.".into(),
        params: json!({"type":"object","properties":{"result": caps.result_schema},"required":["result"]}),
    });
    let tok = &shared.cancel;
    let mut eng = Engine::<R>::new(cfg.clone(), caps.result_schema.clone(), None, Instant::now());
    let mut inflight: Vec<BoxFut<Done>> = vec![];
    eng.start(Instant::now(), prompt);
    loop {
        if tok.is_cancelled() {
            eng.abort(Instant::now());
        }
        // Input that arrived through the handle since the last wake goes into the core's queues.
        {
            let mut q = shared.q.lock().unwrap();
            let now = Instant::now();
            q.steer.drain(..).for_each(|s| eng.steer(now, s));
            q.follow.drain(..).for_each(|f| eng.follow_up(now, f));
        }
        while let Some(a) = eng.poll() {
            match a {
                Action::Event(e) => emit(e),
                Action::Finish(r) => return r,
                Action::CallModel { call_id, messages } => {
                    let req = ModelRequest { session: cfg.id, messages, tools: specs.clone(), cancel: tok.clone() };
                    let fut = model(req);
                    inflight.push(Box::pin(async move { Done::Model(call_id, fut.await) }));
                }
                Action::RunTool { id, name, args } => {
                    let fut: BoxFut<Result<Value, String>> = match caps.tools.iter().find(|t| t.spec.name == name) {
                        Some(t) => (t.f)(ToolCtx { session: cfg.id, call_id: id.clone(), cancel: tok.clone() }, args),
                        None => Box::pin(async move { Err(format!("unknown tool {name}")) }),
                    };
                    inflight.push(Box::pin(async move { Done::Tool(id, fut.await) }));
                }
            }
        }
        if inflight.is_empty() {
            return Err(SessionError::ModelFailed("engine stalled with nothing in flight".into()));
        }
        // Contract kept from session-task: after cancel, the in-flight futures are polled once more.
        let Some(done) = abortable(tok, next_done(&mut inflight)).await else {
            continue; // top of loop sees the token and aborts the engine
        };
        let now = Instant::now();
        {
            let mut q = shared.q.lock().unwrap();
            q.steer.drain(..).for_each(|s| eng.steer(now, s));
            q.follow.drain(..).for_each(|f| eng.follow_up(now, f));
        }
        for d in done {
            match d {
                Done::Model(id, r) => eng.model_done(now, id, r),
                Done::Tool(id, r) => eng.tool_done(now, &id, r),
            }
        }
    }
}
