use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use session_task::*;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// The only thing a host must supply per executor.
pub type Spawner = Arc<dyn Fn(BoxFuture<'static, ()>) + Send + Sync>;

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct Verdict {
    pub score: u32,
    pub label: String,
}
pub fn verdict_schema() -> Value {
    json!({"type":"object","properties":{"score":{"type":"integer"},"label":{"type":"string"}},"required":["score","label"]})
}
pub type Results<R> = mpsc::UnboundedSender<(SessionId, Result<R, SessionError>)>;

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

// ---- scripted fake model ----------------------------------------------------
pub struct Step {
    pub reply: ModelReply,
    pub hold: Option<oneshot::Receiver<()>>,
    pub started: Option<oneshot::Sender<()>>,
}
impl Step {
    pub fn new(reply: ModelReply) -> Self {
        Self { reply, hold: None, started: None }
    }
    pub fn held(mut self, rx: oneshot::Receiver<()>, started: oneshot::Sender<()>) -> Self {
        self.hold = Some(rx);
        self.started = Some(started);
        self
    }
}
pub struct Script {
    steps: Mutex<VecDeque<Step>>,
    pub seen: Mutex<Vec<Vec<Message>>>,
}
pub fn scripted(steps: Vec<Step>) -> (Arc<Script>, impl Fn(ModelRequest) -> BoxFuture<'static, Result<ModelReply, String>> + Send + Sync + 'static) {
    let script = Arc::new(Script { steps: Mutex::new(steps.into()), seen: Mutex::new(vec![]) });
    let s2 = script.clone();
    (script, move |req: ModelRequest| {
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
            Ok(step.reply)
        })
    })
}

// ---- basic caps -------------------------------------------------------------
#[derive(Default)]
pub struct Probe {
    pub started: Mutex<Option<oneshot::Sender<()>>>,
    pub saw_cancel: std::sync::atomic::AtomicBool,
}
#[derive(Deserialize)]
struct AddIn {
    a: i64,
    b: i64,
}

pub fn basic_caps<F>(model: F, results: Results<Verdict>, probe: Arc<Probe>) -> Capabilities<Verdict>
where
    F: Fn(ModelRequest) -> BoxFuture<'static, Result<ModelReply, String>> + Send + Sync + 'static,
{
    Capabilities::<Verdict>::new(verdict_schema())
        .model(model)
        .tool_typed("add", "add two ints", json!({}), |_ctx, i: AddIn| async move { Ok(i.a + i.b) })
        .tool("echo", "echo", json!({}), |_ctx, v| async move { Ok(v) })
        .tool("hang", "never returns", json!({}), move |ctx, _v| {
            let probe = probe.clone();
            async move {
                let p2 = probe.clone();
                ctx.cancel.on_cancel(move || p2.saw_cancel.store(true, std::sync::atomic::Ordering::SeqCst));
                if let Some(tx) = probe.started.lock().unwrap().take() {
                    let _ = tx.send(());
                }
                std::future::pending::<Result<Value, String>>().await
            }
        })
        .on_result(move |id, r| {
            let results = results.clone();
            async move {
                let _ = results.unbounded_send((id, r));
            }
        })
}

/// Start a session and hand its future to the host's executor.
pub fn launch<R: serde::de::DeserializeOwned + Send + 'static>(
    spawn: &Spawner,
    caps: Capabilities<R>,
    cfg: SessionConfig,
    prompt: &str,
) -> (SessionHandle, oneshot::Receiver<SessionEnd>) {
    let (h, fut) = Session::start(caps, cfg, prompt);
    let (tx, rx) = oneshot::channel();
    spawn(Box::pin(async move {
        let _ = tx.send(fut.await);
    }));
    (h, rx)
}
