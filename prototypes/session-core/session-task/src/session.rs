use crate::*;
use serde_json::json;
use std::collections::VecDeque;
use std::future::poll_fn;
use std::pin::pin;
use std::sync::Mutex;
use std::task::Poll;

pub const FINAL_ANSWER: &str = "final_answer";

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

/// Minimal join_all so the library needs no futures crate.
async fn join_all<T>(futs: Vec<BoxFut<T>>) -> Vec<T> {
    let mut slots: Vec<(Option<BoxFut<T>>, Option<T>)> = futs.into_iter().map(|f| (Some(f), None)).collect();
    poll_fn(|cx| {
        let mut pending = false;
        for (f, out) in slots.iter_mut() {
            if let Some(fut) = f {
                match fut.as_mut().poll(cx) {
                    Poll::Ready(v) => {
                        *out = Some(v);
                        *f = None;
                    }
                    Poll::Pending => pending = true,
                }
            }
        }
        if pending { Poll::Pending } else { Poll::Ready(()) }
    })
    .await;
    slots.into_iter().map(|(_, o)| o.unwrap()).collect()
}

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
    let mut msgs = vec![Message::User(prompt)];
    let (mut answer, mut rejected, mut nudges): (Option<R>, u32, u32) = (None, 0, 0);
    let pop = |which: fn(&mut Queues) -> &mut VecDeque<String>| which(&mut shared.q.lock().unwrap()).pop_front();

    for turn in 0..cfg.max_turns {
        if tok.is_cancelled() {
            return Err(SessionError::Aborted);
        }
        while let Some(s) = pop(|q| &mut q.steer) {
            emit(Event::Injected { session: cfg.id, kind: "steer" });
            msgs.push(Message::User(s));
        }
        emit(Event::TurnStart { session: cfg.id, turn });
        let req = ModelRequest { session: cfg.id, messages: msgs.clone(), tools: specs.clone(), cancel: tok.clone() };
        let reply = abortable(tok, model(req))
            .await
            .ok_or(SessionError::Aborted)?
            .map_err(SessionError::ModelFailed)?;
        emit(Event::Tokens { session: cfg.id, tokens: reply.tokens });
        msgs.push(Message::Assistant { text: reply.text.clone(), tool_calls: reply.tool_calls.clone() });

        if reply.tool_calls.is_empty() {
            if let Some(f) = pop(|q| &mut q.follow) {
                emit(Event::Injected { session: cfg.id, kind: "follow_up" });
                msgs.push(Message::User(f));
                continue;
            }
            if let Some(a) = answer {
                return Ok(a);
            }
            if nudges < cfg.max_nudges {
                nudges += 1;
                msgs.push(Message::User(format!("Finish by calling {FINAL_ANSWER}.")));
                continue;
            }
            return Err(SessionError::NoAnswer);
        }

        // Tool batch runs concurrently inside this one task (no spawning).
        let mut futs: Vec<BoxFut<(String, Result<Value, String>)>> = vec![];
        for c in &reply.tool_calls {
            let (id, name, args) = (c.id.clone(), c.name.clone(), c.args.clone());
            if name == FINAL_ANSWER {
                let r = args.get("result").cloned().unwrap_or(Value::Null);
                futs.push(Box::pin(async move { (id, Ok(r)) }));
                continue;
            }
            emit(Event::ToolStart { session: cfg.id, name: name.clone() });
            match caps.tools.iter().find(|t| t.spec.name == name) {
                Some(t) => {
                    let fut = (t.f)(ToolCtx { session: cfg.id, call_id: id.clone(), cancel: tok.clone() }, args);
                    futs.push(Box::pin(async move { (id, fut.await) }));
                }
                None => futs.push(Box::pin(async move { (id, Err(format!("unknown tool {name}"))) })),
            }
        }
        let results = abortable(tok, join_all(futs)).await.ok_or(SessionError::Aborted)?;
        for (call, (_, res)) in reply.tool_calls.iter().zip(results) {
            let (content, is_error) = if call.name == FINAL_ANSWER {
                match serde_json::from_value::<R>(res.clone().unwrap_or(Value::Null)) {
                    Ok(v) => {
                        answer = Some(v);
                        ("accepted".to_string(), false)
                    }
                    Err(e) => {
                        rejected += 1;
                        emit(Event::ResultRejected { session: cfg.id, error: e.to_string() });
                        if rejected > cfg.max_result_retries {
                            return Err(SessionError::Malformed {
                                attempts: rejected,
                                last_error: e.to_string(),
                                raw: res.unwrap_or(Value::Null),
                            });
                        }
                        (format!("rejected: {e}. Expected schema: {}", caps.result_schema), true)
                    }
                }
            } else {
                let (c, e) = match res {
                    Ok(v) => (v.to_string(), false),
                    Err(s) => (s, true),
                };
                emit(Event::ToolEnd { session: cfg.id, name: call.name.clone(), is_error: e });
                (c, e)
            };
            msgs.push(Message::ToolResult { call_id: call.id.clone(), name: call.name.clone(), content, is_error });
        }
        // Accepted answer: stop unless input is queued (steer is injected at the top of
        // the loop; a follow-up is injected here because the session would otherwise end).
        if answer.is_some() && shared.q.lock().unwrap().steer.is_empty() {
            match pop(|q| &mut q.follow) {
                Some(f) => {
                    emit(Event::Injected { session: cfg.id, kind: "follow_up" });
                    msgs.push(Message::User(f));
                }
                None => return Ok(answer.take().unwrap()),
            }
        }
    }
    Err(SessionError::MaxTurns)
}
