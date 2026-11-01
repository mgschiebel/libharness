#![allow(unused_must_use)]
//! Every scenario is `async fn(Spawner)`. The tests call the same functions from
//! tokio and from futures-executor; nothing in here changes between them.
use crate::glue::*;
use futures::channel::{mpsc, oneshot};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use session_task::*;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};

fn good() -> Value {
    json!({"score": 7, "label": "ok"})
}
fn user_texts(m: &[Message]) -> Vec<String> {
    m.iter().filter_map(|m| if let Message::User(s) = m { Some(s.clone()) } else { None }).collect()
}

/// (1,5,8) prompt, typed tool, typed result.
pub async fn typed_result(spawn: Spawner) {
    let (rtx, mut rrx) = mpsc::unbounded();
    let (script, model) = scripted(vec![
        Step::new(calls(vec![call("c1", "add", json!({"a": 2, "b": 3}))])),
        Step::new(answer(good())),
    ]);
    let (_h, end) = launch(&spawn, basic_caps(model, rtx, Default::default()), SessionConfig::new(1), "go");
    let (id, r) = rrx.next().await.unwrap();
    assert_eq!(id, SessionId(1));
    assert_eq!(r, Ok(Verdict { score: 7, label: "ok".into() }));
    assert_eq!(end.await.unwrap(), SessionEnd::Completed);
    let seen = script.seen.lock().unwrap();
    assert!(matches!(&seen[1][2], Message::ToolResult { content, .. } if content == "5"));
}

/// (5) malformed answers: fed back, recovered; exhausted; never answered; String result.
pub async fn malformed_results(spawn: Spawner) {
    // recover after one bad answer
    let (rtx, mut rrx) = mpsc::unbounded();
    let (script, model) = scripted(vec![
        Step::new(answer(json!({"score": "high"}))),
        Step::new(answer(good())),
    ]);
    launch(&spawn, basic_caps(model, rtx, Default::default()), SessionConfig::new(1), "go");
    assert!(rrx.next().await.unwrap().1.is_ok());
    let seen = script.seen.lock().unwrap();
    let Message::ToolResult { content, is_error, .. } = &seen[1][2] else { panic!() };
    assert!(*is_error && content.starts_with("rejected:") && content.contains("Expected schema"), "{content}");
    drop(seen);

    // always malformed: 1 try + 2 retries then Err(Malformed) delivered to the host
    let (rtx, mut rrx) = mpsc::unbounded();
    let (_s, model) = scripted((0..5).map(|_| Step::new(answer(json!({"nope": 1})))).collect());
    let (_h, end) = launch(&spawn, basic_caps(model, rtx, Default::default()), SessionConfig::new(2), "go");
    match rrx.next().await.unwrap().1 {
        Err(SessionError::Malformed { attempts, raw, .. }) => {
            assert_eq!(attempts, 3);
            assert_eq!(raw, json!({"nope": 1}));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(end.await.unwrap(), SessionEnd::Failed);

    // model only chats: one nudge, then NoAnswer
    let (rtx, mut rrx) = mpsc::unbounded();
    let (script, model) = scripted(vec![Step::new(say("hi")), Step::new(say("still hi"))]);
    launch(&spawn, basic_caps(model, rtx, Default::default()), SessionConfig::new(3), "go");
    assert_eq!(rrx.next().await.unwrap().1, Err(SessionError::NoAnswer));
    assert_eq!(script.seen.lock().unwrap().len(), 2);

    // R = String: the "typed value" can just be a string
    let (stx, mut srx) = mpsc::unbounded::<(SessionId, Result<String, SessionError>)>();
    let (_s, model) = scripted(vec![Step::new(answer(json!("plain text")))]);
    let caps = Capabilities::<String>::new(json!({"type":"string"}))
        .model(model)
        .on_result(move |id, r| {
            let stx = stx.clone();
            async move {
                let _ = stx.unbounded_send((id, r));
            }
        });
    launch(&spawn, caps, SessionConfig::new(4), "go");
    assert_eq!(srx.next().await.unwrap().1, Ok("plain text".to_string()));
}

/// (2) steering lands at the next turn boundary; follow-up only when it would stop.
pub async fn steer_follow_up(spawn: Spawner) {
    let (rtx, mut rrx) = mpsc::unbounded();
    let (gate_tx, gate_rx) = oneshot::channel();
    let (started_tx, started_rx) = oneshot::channel();
    let (script, model) = scripted(vec![
        Step::new(calls(vec![call("c1", "echo", json!("x"))])).held(gate_rx, started_tx),
        Step::new(answer(json!({"score": 1, "label": "first"}))),
        Step::new(answer(json!({"score": 2, "label": "after follow-up"}))),
    ]);
    let (h, end) = launch(&spawn, basic_caps(model, rtx, Default::default()), SessionConfig::new(1), "go");
    started_rx.await.unwrap(); // model call 1 is in flight
    assert!(h.steer("steer-1"));
    assert!(h.follow_up("follow-1"));
    gate_tx.send(()).unwrap();
    let (_, r) = rrx.next().await.unwrap();
    assert_eq!(r.unwrap().label, "after follow-up");
    assert_eq!(end.await.unwrap(), SessionEnd::Completed);
    let seen = script.seen.lock().unwrap();
    assert_eq!(user_texts(&seen[0]), ["go"]);
    assert_eq!(user_texts(&seen[1]), ["go", "steer-1"]); // after turn 1, not mid-call
    assert_eq!(user_texts(&seen[2]), ["go", "steer-1", "follow-1"]);
    assert!(!h.steer("late"), "finished session refuses input");
}

/// (3) two sessions, same executor, no shared state; B finishes while A is still held.
pub async fn parallel_sessions(spawn: Spawner) {
    let (rtx, mut rrx) = mpsc::unbounded();
    let (a_gate, a_rx) = oneshot::channel();
    let (a_started, a_started_rx) = oneshot::channel();
    let (_sa, ma) = scripted(vec![Step::new(answer(json!({"score": 1, "label": "A"}))).held(a_rx, a_started)]);
    let (_sb, mb) = scripted(vec![Step::new(answer(json!({"score": 2, "label": "B"})))]);
    let (_ha, ea) = launch(&spawn, basic_caps(ma, rtx.clone(), Default::default()), SessionConfig::new(10), "a");
    a_started_rx.await.unwrap();
    let (_hb, eb) = launch(&spawn, basic_caps(mb, rtx, Default::default()), SessionConfig::new(11), "b");
    let (id, r) = rrx.next().await.unwrap();
    assert_eq!((id, r.unwrap().label), (SessionId(11), "B".to_string())); // B first
    assert_eq!(eb.await.unwrap(), SessionEnd::Completed);
    a_gate.send(()).unwrap();
    let (id, r) = rrx.next().await.unwrap();
    assert_eq!((id, r.unwrap().label), (SessionId(10), "A".to_string()));
    assert_eq!(ea.await.unwrap(), SessionEnd::Completed);
}

/// (2,7) abort while a tool is in flight: hook runs, terminal callback says Aborted.
pub async fn abort_running(spawn: Spawner) {
    let (rtx, mut rrx) = mpsc::unbounded();
    let probe: Arc<Probe> = Default::default();
    let (st_tx, st_rx) = oneshot::channel();
    *probe.started.lock().unwrap() = Some(st_tx);
    let (_s, model) = scripted(vec![Step::new(calls(vec![call("c", "hang", json!({}))]))]);
    let (h, end) = launch(&spawn, basic_caps(model, rtx, probe.clone()), SessionConfig::new(1), "go");
    st_rx.await.unwrap();
    h.abort();
    assert_eq!(rrx.next().await.unwrap().1, Err(SessionError::Aborted));
    assert_eq!(end.await.unwrap(), SessionEnd::Aborted);
    assert!(probe.saw_cancel.load(SeqCst), "on_cancel hook ran");
}

// ---- spawn as an ordinary callback (6, 7) -----------------------------------
#[derive(Deserialize)]
struct SpawnIn {
    task: String,
}
#[derive(Serialize)]
struct SpawnOut {
    answer: String,
}

/// Host-owned state for the whole tree. The library sees none of it.
pub struct Host {
    spawn: Spawner,
    inline: bool,
    max_depth: u32,
    max_fanout: usize,
    next_id: AtomicU64,
    hang_started: Mutex<Option<oneshot::Sender<()>>>,
    ledger: mpsc::UnboundedSender<(u64, u32, String)>,
    handles: Mutex<Vec<SessionHandle>>,
    max_depth_seen: AtomicU32,
    /// leaves wait on this (if set) until the fan-out refusal releases it: makes the test deterministic
    gate: Mutex<Option<oneshot::Sender<()>>>,
    gate_rx: Option<futures::future::Shared<oneshot::Receiver<()>>>,
}
type Report = Arc<Mutex<Option<oneshot::Sender<Result<String, SessionError>>>>>;

fn tree_model(host: Arc<Host>, depth: u32) -> impl Fn(ModelRequest) -> futures::future::BoxFuture<'static, Result<ModelReply, String>> + Send + Sync + 'static {
    move |req| {
        let host = host.clone();
        Box::pin(async move {
            let task = user_texts(&req.messages)[0].clone();
            let results: Vec<String> = req
                .messages
                .iter()
                .filter_map(|m| if let Message::ToolResult { name, content, .. } = m {
                    // spawn_agent returns {"answer": ..}; errors are plain strings
                    (name != "final_answer").then(|| serde_json::from_str::<Value>(content).ok().and_then(|v| v["answer"].as_str().map(String::from)).unwrap_or(content.clone()))
                } else { None })
                .collect();
            if !results.is_empty() {
                return Ok(answer(json!(format!("d{depth}[{}]", results.join("|")))));
            }
            Ok(match task.as_str() {
                "recurse" | "delegate-hang" => calls(vec![call("s", "spawn_agent", json!({"task": if task == "recurse" { "recurse" } else { "hang" }}))]),
                "fan" => calls((0..3).map(|i| call(&format!("s{i}"), "spawn_agent", json!({"task": "leaf"}))).collect()),
                "hang" => calls(vec![call("h", "hang", json!({}))]),
                _ => {
                    if let Some(g) = &host.gate_rx {
                        let _ = g.clone().await;
                    }
                    answer(json!(format!("leaf@{depth}")))
                }
            })
        })
    }
}

/// Capabilities for one session at `depth`. The spawn tool is registered exactly like
/// any other tool; the depth counter and fan-out limit live in this closure.
fn tree_caps(host: Arc<Host>, depth: u32, report: Option<Report>, my_id: u64) -> Capabilities<String> {
    host.max_depth_seen.fetch_max(depth, SeqCst);
    let live = Arc::new(AtomicUsize::new(0));
    let h2 = host.clone();
    Capabilities::<String>::new(json!({"type":"string"}))
        .model(tree_model(host.clone(), depth))
        .tool("hang", "never returns", json!({}), {
            let host = host.clone();
            move |_ctx, _v| {
                let host = host.clone();
                async move {
                    if let Some(tx) = host.hang_started.lock().unwrap().take() {
                        let _ = tx.send(());
                    }
                    std::future::pending::<Result<Value, String>>().await
                }
            }
        })
        .tool_typed("spawn_agent", "run a sub-agent", json!({}), move |ctx, inp: SpawnIn| {
            let (host, live) = (host.clone(), live.clone());
            async move {
                if depth >= host.max_depth {
                    return Err("depth limit reached".to_string());
                }
                if live.fetch_add(1, SeqCst) >= host.max_fanout {
                    live.fetch_sub(1, SeqCst);
                    if let Some(tx) = host.gate.lock().unwrap().take() {
                        let _ = tx.send(());
                    }
                    return Err("fan-out limit reached".to_string());
                }
                struct Dec(Arc<AtomicUsize>);
                impl Drop for Dec {
                    fn drop(&mut self) {
                        self.0.fetch_sub(1, SeqCst);
                    }
                }
                let _dec = Dec(live);
                let id = host.next_id.fetch_add(1, SeqCst);
                let (otx, orx) = oneshot::channel();
                let child = tree_caps(host.clone(), depth + 1, Some(Arc::new(Mutex::new(Some(otx)))), id);
                let (ch, fut) = Session::start(child, SessionConfig::new(id), inp.task);
                host.handles.lock().unwrap().push(ch.clone());
                // Cancellation: the library handed us ctx.cancel; we wire it to the child.
                ctx.cancel.on_cancel(move || ch.abort());
                if host.inline {
                    fut.await; // child lives inside this callback's future
                } else {
                    (host.spawn)(Box::pin(async move {
                        fut.await;
                    }));
                }
                match orx.await {
                    Ok(Ok(s)) => Ok(SpawnOut { answer: s }),
                    Ok(Err(e)) => Err(format!("child failed: {e:?}")),
                    Err(_) => Err("child vanished".to_string()),
                }
            }
        })
        .on_result(move |_sid, r| {
            let (host, report) = (h2.clone(), report.clone());
            async move {
                let _ = host.ledger.unbounded_send((my_id, depth, format!("{r:?}")));
                if let Some(tx) = report.and_then(|r| r.lock().unwrap().take()) {
                    let _ = tx.send(r);
                }
            }
        })
}

struct Tree {
    host: Arc<Host>,
    ledger: mpsc::UnboundedReceiver<(u64, u32, String)>,
    results: mpsc::UnboundedReceiver<(SessionId, Result<String, SessionError>)>,
    root: SessionHandle,
    end: oneshot::Receiver<SessionEnd>,
}
fn start_tree(spawn: Spawner, inline: bool, max_depth: u32, max_fanout: usize, task: &str, hang: Option<oneshot::Sender<()>>) -> Tree {
    use futures::FutureExt;
    let (gtx, grx) = oneshot::channel();
    let gated = task == "fan";
    let (ledger_tx, ledger) = mpsc::unbounded();
    let host = Arc::new(Host {
        spawn: spawn.clone(), inline, max_depth, max_fanout, next_id: AtomicU64::new(1),
        hang_started: Mutex::new(hang), ledger: ledger_tx, handles: Mutex::new(vec![]), max_depth_seen: AtomicU32::new(0),
        gate: Mutex::new(gated.then_some(gtx)), gate_rx: gated.then(|| grx.shared()),
    });
    let (rtx, results) = mpsc::unbounded();
    let (otx, orx) = oneshot::channel();
    // root reports through the same path as children, then we forward to `results`
    let caps = tree_caps(host.clone(), 0, Some(Arc::new(Mutex::new(Some(otx)))), 0);
    let (root, end) = launch(&spawn, caps, SessionConfig::new(0), task);
    spawn(Box::pin(async move {
        if let Ok(r) = orx.await {
            let _ = rtx.unbounded_send((SessionId(0), r));
        }
    }));
    Tree { host, ledger, results, root, end }
}

/// (4,6) three levels of children, host depth limit refuses the fourth; library has no parent concept.
pub async fn spawn_depth_limit(spawn: Spawner) {
    let mut t = start_tree(spawn, false, 2, 10, "recurse", None);
    let (_, r) = t.results.next().await.unwrap();
    let r = r.unwrap();
    assert_eq!(r, "d0[d1[d2[depth limit reached]]]");
    assert_eq!(t.end.await.unwrap(), SessionEnd::Completed);
    assert_eq!(t.host.max_depth_seen.load(SeqCst), 2);
    let mut n = 0;
    while t.ledger.try_recv().is_ok() {
        n += 1;
    }
    assert_eq!(n, 3);
}

/// (6) fan-out limit enforced by the host callback, in one batch of 3 parallel spawns.
pub async fn spawn_fanout_limit(spawn: Spawner) {
    let mut t = start_tree(spawn, false, 3, 2, "fan", None);
    let (_, r) = t.results.next().await.unwrap();
    let s = r.unwrap();
    assert_eq!(s.matches("leaf@1").count(), 2, "{s}");
    assert_eq!(s.matches("fan-out limit reached").count(), 1, "{s}");
}

/// (7) parent aborted while awaiting a host-spawned child: the on_cancel hook aborts
/// the child, which reports Aborted through its own terminal callback.
pub async fn cancel_reaches_spawned_child(spawn: Spawner) {
    let (st_tx, st_rx) = oneshot::channel();
    let mut t = start_tree(spawn, false, 3, 3, "delegate-hang", Some(st_tx));
    st_rx.await.unwrap(); // grandchild-level tool is hanging
    t.root.abort();
    assert_eq!(t.results.next().await.unwrap().1, Err(SessionError::Aborted));
    assert_eq!(t.end.await.unwrap(), SessionEnd::Aborted);
    let mut got = vec![];
    while got.len() < 2 {
        got.push(t.ledger.next().await.unwrap());
    }
    got.sort();
    assert_eq!(got[0].2, "Err(Aborted)");
    assert_eq!(got[1], (1, 1, "Err(Aborted)".to_string()));
}

/// (7) the inline variant: the child future lives inside the parent's callback future.
/// The hook cancels the child's token, and because the library polls the in-flight
/// callback once more after cancel (see `abortable`), the child still reports Aborted
/// before everything is dropped. Without that final poll it would vanish silently.
pub async fn cancel_inline_child(spawn: Spawner) {
    let (st_tx, st_rx) = oneshot::channel();
    let mut t = start_tree(spawn, true, 3, 3, "delegate-hang", Some(st_tx));
    st_rx.await.unwrap();
    t.root.abort();
    assert_eq!(t.results.next().await.unwrap().1, Err(SessionError::Aborted));
    assert_eq!(t.end.await.unwrap(), SessionEnd::Aborted);
    assert!(t.host.handles.lock().unwrap()[0].is_aborted(), "hook fired");
    let mut got = vec![];
    while got.len() < 2 {
        got.push(t.ledger.next().await.unwrap());
    }
    assert!(got.iter().all(|g| g.2 == "Err(Aborted)"));
}
