//! The host's natural callbacks, written once against `Api` and run under every boundary.
//! `Api` is what a callback sees of the library: B1 builds it from real handles (token with
//! hooks, sink), B2 and B3 from plain callbacks (`cancelled()`, `emit()`), so a callback can
//! only use a hook where the boundary has one (`on_cancel` returns false elsewhere).
use cb_core::b1::Sink;
use cb_core::*;
use futures::channel::oneshot;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

#[derive(Clone)]
pub struct Api {
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    emit: Arc<dyn Fn(Ev) + Send + Sync>,
    pub token: Option<CancelToken>,
}
impl Api {
    pub fn from_handles(token: CancelToken, sink: Sink) -> Api {
        let t = token.clone();
        Api { cancelled: Arc::new(move || t.is_cancelled()), emit: sink, token: Some(token) }
    }
    pub fn from_plain(cancelled: Arc<dyn Fn() -> bool + Send + Sync>, emit: Arc<dyn Fn(Ev) + Send + Sync>) -> Api {
        Api { cancelled, emit, token: None }
    }
    pub fn cancelled(&self) -> bool {
        (self.cancelled)()
    }
    pub fn emit_delta(&self, s: &str) {
        (self.emit)(Ev::Delta(s.to_string()))
    }
    /// Release-on-cancel hook. Only a boundary that passes a real token has one.
    pub fn on_cancel(&self, f: impl FnOnce() + Send + 'static) -> bool {
        match &self.token {
            Some(t) => {
                t.on_cancel(f);
                true
            }
            None => false,
        }
    }
}

pub type HModel = Arc<dyn Fn(ModelRequest, Api) -> BoxFut<Result<ModelReply, String>> + Send + Sync>;
pub type HTool = Arc<dyn Fn(Value, Api) -> BoxFut<Result<Value, String>> + Send + Sync>;
pub type HTransport = Arc<dyn Fn(HttpReq, Api) -> BoxFut<Result<(HttpHead, Box<dyn BodyStream>), String>> + Send + Sync>;

/// What a host registers: the model, tools (name, description, callback), an optional
/// transport, and the typed-result validator with its schema.
#[derive(Clone)]
pub struct HostCaps {
    pub model: HModel,
    pub tools: Vec<(String, String, HTool)>,
    pub transport: Option<HTransport>,
    pub validate: Arc<dyn Fn(&Value) -> Result<(), String> + Send + Sync>,
    pub schema: Value,
}
impl HostCaps {
    pub fn new(model: HModel) -> Self {
        HostCaps {
            model,
            tools: vec![],
            transport: None,
            validate: Arc::new(|v| serde_json::from_value::<Verdict>(v.clone()).map(|_| ()).map_err(|e| e.to_string())),
            schema: verdict_schema(),
        }
    }
    pub fn tool(mut self, name: &str, desc: &str, f: impl Fn(Value, Api) -> BoxFut<Result<Value, String>> + Send + Sync + 'static) -> Self {
        self.tools.push((name.into(), desc.into(), Arc::new(f)));
        self
    }
    pub fn spec(&self) -> Spec {
        Spec {
            tools: self.tools.iter().map(|(n, d, _)| ToolSpec { name: n.clone(), description: d.clone(), params: json!({}) }).collect(),
            result_schema: self.schema.clone(),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Clone)]
pub struct Verdict {
    pub score: u32,
    pub label: String,
}
pub fn verdict_schema() -> Value {
    json!({"type":"object","properties":{"score":{"type":"integer"},"label":{"type":"string"}},"required":["score","label"]})
}

// ---- message helpers and the scripted model ------------------------------------------------
pub fn call(id: &str, name: &str, args: Value) -> ToolCall {
    ToolCall { id: id.into(), name: name.into(), args }
}
pub fn calls(c: Vec<ToolCall>) -> ModelReply {
    ModelReply { text: String::new(), tool_calls: c, tokens: 10 }
}
pub fn say(t: &str) -> ModelReply {
    ModelReply { text: t.into(), tool_calls: vec![], tokens: 5 }
}
pub fn answer(v: Value) -> ModelReply {
    calls(vec![call("fa", "final_answer", json!({ "result": v }))])
}
pub fn good() -> Value {
    json!({"score": 7, "label": "ok"})
}
pub fn user_texts(m: &[Message]) -> Vec<String> {
    m.iter().filter_map(|m| if let Message::User(s) = m { Some(s.clone()) } else { None }).collect()
}

pub struct Turn {
    pub reply: ModelReply,
    pub hold: Option<oneshot::Receiver<()>>,
    pub started: Option<oneshot::Sender<()>>,
    /// stream these words through `api.emit_delta`, `delay` apart, before replying
    pub stream: Vec<String>,
    pub delay: Duration,
}
impl Turn {
    pub fn new(reply: ModelReply) -> Self {
        Self { reply, hold: None, started: None, stream: vec![], delay: Duration::ZERO }
    }
    pub fn held(mut self, rx: oneshot::Receiver<()>, started: oneshot::Sender<()>) -> Self {
        self.hold = Some(rx);
        self.started = Some(started);
        self
    }
    pub fn streaming(mut self, words: &[&str], delay_ms: u64) -> Self {
        self.stream = words.iter().map(|s| s.to_string()).collect();
        self.delay = Duration::from_millis(delay_ms);
        self
    }
}
pub struct Script {
    steps: Mutex<VecDeque<Turn>>,
    pub seen: Mutex<Vec<Vec<Message>>>,
}
pub fn scripted(steps: Vec<Turn>) -> (Arc<Script>, HModel) {
    let script = Arc::new(Script { steps: Mutex::new(steps.into()), seen: Mutex::new(vec![]) });
    let s2 = script.clone();
    (
        script,
        Arc::new(move |req: ModelRequest, api: Api| {
            let s = s2.clone();
            Box::pin(async move {
                s.seen.lock().unwrap().push(req.messages);
                let step = s.steps.lock().unwrap().pop_front();
                let Some(mut step) = step else { return Err("script exhausted".to_string()) };
                if let Some(tx) = step.started.take() {
                    let _ = tx.send(());
                }
                if let Some(rx) = step.hold.take() {
                    let _ = rx.await;
                }
                for w in &step.stream {
                    delay(step.delay).await;
                    api.emit_delta(w);
                }
                Ok(step.reply)
            })
        }),
    )
}

// ---- a timer (the library has none; the host brings one) ---------------------------------
pub struct Delay {
    st: Arc<Mutex<(bool, Option<Waker>)>>,
}
pub fn delay(d: Duration) -> Delay {
    let st = Arc::new(Mutex::new((false, None::<Waker>)));
    let s2 = st.clone();
    std::thread::spawn(move || {
        std::thread::sleep(d);
        let mut g = s2.lock().unwrap();
        g.0 = true;
        if let Some(w) = g.1.take() {
            w.wake();
        }
    });
    Delay { st }
}
impl Future for Delay {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut g = self.st.lock().unwrap();
        if g.0 {
            Poll::Ready(())
        } else {
            g.1 = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

// ---- standard tools -----------------------------------------------------------------------
#[derive(Default)]
pub struct Probe {
    pub started: Mutex<Option<oneshot::Sender<()>>>,
    /// the hanging tool's future was dropped (the boundary released it)
    pub dropped: AtomicBool,
    pub saw_cancel: AtomicBool,
}
struct OnDrop(Arc<Probe>);
impl Drop for OnDrop {
    fn drop(&mut self) {
        self.0.dropped.store(true, SeqCst);
    }
}

pub fn basic_caps(model: HModel, probe: Arc<Probe>) -> HostCaps {
    HostCaps::new(model)
        .tool("add", "add two ints", |v, _| Box::pin(async move { Ok(json!(v["a"].as_i64().unwrap_or(0) + v["b"].as_i64().unwrap_or(0))) }))
        .tool("echo", "echo", |v, _| Box::pin(async move { Ok(v) }))
        .tool("hang", "never returns", move |_v, api| {
            let probe = probe.clone();
            Box::pin(async move {
                let _g = OnDrop(probe.clone());
                let p2 = probe.clone();
                api.on_cancel(move || p2.saw_cancel.store(true, SeqCst));
                if let Some(tx) = probe.started.lock().unwrap().take() {
                    let _ = tx.send(());
                }
                std::future::pending::<Result<Value, String>>().await
            })
        })
}

/// What a host observer records.
#[derive(Clone, Default)]
pub struct Obs(pub Arc<Mutex<Vec<(std::time::Instant, Ev)>>>);
impl Obs {
    pub fn sink(&self) -> Arc<dyn Fn(Ev) + Send + Sync> {
        let o = self.0.clone();
        Arc::new(move |e| o.lock().unwrap().push((std::time::Instant::now(), e)))
    }
    pub fn deltas(&self) -> Vec<(std::time::Instant, String)> {
        self.0.lock().unwrap().iter().filter_map(|(t, e)| if let Ev::Delta(s) = e { Some((*t, s.clone())) } else { None }).collect()
    }
}
