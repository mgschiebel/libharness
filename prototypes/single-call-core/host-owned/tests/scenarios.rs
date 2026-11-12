//! OWNER 1: the host's own plain code (`host_owned::run`). No executor, no futures crate in
//! this file, no async. Steer "arrives while a call is in flight" by being pushed from inside
//! the fake model; abort and the spawned child use real `std::thread`s only as the *test's*
//! way to deliver the abort, never inside the loop.
use call_core::*;
use host_owned::{run, Input, Wiring};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Seen = Arc<Mutex<Vec<Vec<Message>>>>;

fn good() -> Value {
    json!({"score": 7, "label": "ok"})
}
fn call(id: &str, name: &str, args: Value) -> ToolCall {
    ToolCall { id: id.into(), name: name.into(), args }
}
fn calls(c: Vec<ToolCall>) -> ModelReply {
    ModelReply { text: String::new(), tool_calls: c, tokens: 10 }
}
fn say(t: &str) -> ModelReply {
    ModelReply { text: t.into(), tool_calls: vec![], tokens: 5 }
}
fn answer(v: Value) -> ModelReply {
    calls(vec![call("fa", "final_answer", json!({ "result": v }))])
}
fn user_texts(m: &[Message]) -> Vec<String> {
    m.iter().filter_map(|m| if let Message::User(s) = m { Some(s.clone()) } else { None }).collect()
}
fn verdict_ok(v: &Value) -> Result<(), String> {
    (v["score"].is_u64() && v["label"].is_string()).then_some(()).ok_or_else(|| format!("bad verdict: {v}"))
}
fn spec() -> Spec {
    let t = |n: &str| ToolSpec { name: n.into(), description: n.into(), params: json!({}) };
    Spec { tools: vec![t("add"), t("echo")], result_schema: json!({"type":"object"}) }
}
fn basic_tool(c: &ToolCall, _t: &CancelToken) -> Result<Value, String> {
    match c.name.as_str() {
        "add" => Ok(json!(c.args["a"].as_i64().unwrap() + c.args["b"].as_i64().unwrap())),
        _ => Ok(c.args.clone()),
    }
}
/// Plain scripted model. `on_call(n)` lets a test do something "during" call n.
fn scripted(replies: Vec<ModelReply>, mut on_call: impl FnMut(usize) + 'static) -> (Seen, impl FnMut(ModelRequest, &CancelToken) -> Result<ModelReply, String>) {
    let seen: Seen = Default::default();
    let (s2, mut q, mut n) = (seen.clone(), std::collections::VecDeque::from(replies), 0);
    (seen, move |req: ModelRequest, _t: &CancelToken| {
        s2.lock().unwrap().push(req.messages);
        on_call(n);
        n += 1;
        q.pop_front().ok_or_else(|| "script exhausted".to_string())
    })
}
fn go(
    replies: Vec<ModelReply>,
    cfg: Config,
    validate: &dyn Fn(&Value) -> Result<(), String>,
    mailbox: &mut dyn FnMut() -> Vec<Input>,
    on_call: impl FnMut(usize) + 'static,
) -> (Seen, State, Result<Value, SessionError>) {
    let (seen, mut model) = scripted(replies, on_call);
    let (sp, cancel) = (spec(), CancelToken::new());
    let mut tool = basic_tool;
    let mut on_step = |_: &State| true;
    let mut w = Wiring { spec: &sp, validate, cancel: &cancel, model: &mut model, tool: &mut tool, mailbox, on_step: &mut on_step };
    let (st, r) = run(&mut w, start(SessionId(1), cfg, "go"));
    (seen, st, r)
}
fn plain(replies: Vec<ModelReply>) -> (Seen, State, Result<Value, SessionError>) {
    go(replies, Config::default(), &verdict_ok, &mut Vec::new, |_| {})
}

#[test]
fn typed_result() {
    let (seen, st, r) = plain(vec![calls(vec![call("c1", "add", json!({"a": 2, "b": 3}))]), answer(good())]);
    assert_eq!(r, Ok(good()));
    assert_eq!(st.phase, Phase::Done);
    assert!(matches!(&seen.lock().unwrap()[1][2], Message::ToolResult { content, .. } if content == "5"));
}

#[test]
fn malformed_results() {
    let (seen, _, r) = plain(vec![answer(json!({"score": "high"})), answer(good())]);
    assert!(r.is_ok());
    let Message::ToolResult { content, is_error, .. } = &seen.lock().unwrap()[1][2] else { panic!() };
    assert!(*is_error && content.starts_with("rejected:") && content.contains("Expected schema"));
    let (_, _, r) = plain((0..5).map(|_| answer(json!({"nope": 1}))).collect());
    assert!(matches!(r, Err(SessionError::Malformed { attempts: 3, ref raw, .. }) if *raw == json!({"nope": 1})));
    let (seen, _, r) = plain(vec![say("hi"), say("still hi")]);
    assert_eq!((r, seen.lock().unwrap().len()), (Err(SessionError::NoAnswer), 2));
    let (_, _, r) = go(vec![answer(json!("plain text"))], Config::default(), &|v| v.is_string().then_some(()).ok_or("no".into()), &mut Vec::new, |_| {});
    assert_eq!(r, Ok(json!("plain text")));
}

#[test]
fn steer_follow_up() {
    let mail: Arc<Mutex<Vec<Input>>> = Default::default();
    let m2 = mail.clone();
    let mut mailbox = {
        let mail = mail.clone();
        move || std::mem::take(&mut *mail.lock().unwrap())
    };
    let replies = vec![
        calls(vec![call("c1", "echo", json!("x"))]),
        answer(json!({"score": 1, "label": "first"})),
        answer(json!({"score": 2, "label": "after follow-up"})),
    ];
    // "while model call 1 is in flight": the fake model puts input into the host's mailbox
    let (seen, _, r) = go(replies, Config::default(), &verdict_ok, &mut mailbox, move |n| {
        if n == 0 {
            m2.lock().unwrap().extend([Input::Steer("steer-1".into()), Input::FollowUp("follow-1".into())]);
        }
    });
    assert_eq!(r.unwrap()["label"], "after follow-up");
    let seen = seen.lock().unwrap();
    assert_eq!(user_texts(&seen[0]), ["go"]);
    assert_eq!(user_texts(&seen[1]), ["go", "steer-1"]);
    assert_eq!(user_texts(&seen[2]), ["go", "steer-1", "follow-1"]);
}

#[test]
fn abort_running() {
    // a blocking tool that returns when the token fires (its hook releases it); the test
    // thread delivers the abort. The loop itself is single-threaded plain code.
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let hook_ran = Arc::new(AtomicBool::new(false));
    let cancel = CancelToken::new();
    let (c2, h2) = (cancel.clone(), hook_ran.clone());
    let worker = std::thread::spawn(move || {
        let (_seen, mut model) = scripted(vec![calls(vec![call("c", "hang", json!({}))])], |_| {});
        let sp = Spec { tools: vec![ToolSpec { name: "hang".into(), description: "".into(), params: json!({}) }], result_schema: json!({}) };
        let mut tool = |_: &ToolCall, t: &CancelToken| {
            let (tx, rx) = mpsc::channel::<()>();
            let h = h2.clone();
            t.on_cancel(move || {
                h.store(true, SeqCst);
                let _ = tx.send(());
            });
            started_tx.send(()).unwrap();
            rx.recv_timeout(Duration::from_secs(5)).map(|_| json!("released")).map_err(|_| "never cancelled".to_string())
        };
        let (mut mailbox, mut on_step) = (Vec::new, |_: &State| true);
        let mut w = Wiring { spec: &sp, validate: &verdict_ok, cancel: &c2, model: &mut model, tool: &mut tool, mailbox: &mut mailbox, on_step: &mut on_step };
        run(&mut w, start(SessionId(1), Config::default(), "go")).1
    });
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    cancel.cancel();
    assert_eq!(worker.join().unwrap(), Err(SessionError::Aborted));
    assert!(hook_ran.load(SeqCst));
}

// ---- spawn as an ordinary callback: the tool just calls the host's own function ----------
struct Host {
    max_depth: u32,
    max_children: usize,
    ledger: Mutex<Vec<(u32, String)>>,
    started: Mutex<Option<mpsc::Sender<()>>>,
    max_depth_seen: AtomicUsize,
}

fn tree_reply(req: &ModelRequest, depth: u32) -> ModelReply {
    let task = user_texts(&req.messages)[0].clone();
    let results: Vec<String> = req
        .messages
        .iter()
        .filter_map(|m| match m {
            Message::ToolResult { name, content, .. } if name != "final_answer" => {
                Some(serde_json::from_str::<Value>(content).ok().and_then(|v| v["answer"].as_str().map(String::from)).unwrap_or(content.clone()))
            }
            _ => None,
        })
        .collect();
    if !results.is_empty() {
        return answer(json!(format!("d{depth}[{}]", results.join("|"))));
    }
    match task.as_str() {
        "recurse" => calls(vec![call("s", "spawn_agent", json!({"task": "recurse"}))]),
        "delegate-hang" => calls(vec![call("s", "spawn_agent", json!({"task": "hang"}))]),
        "fan" => calls((0..3).map(|i| call(&format!("s{i}"), "spawn_agent", json!({"task": "leaf"}))).collect()),
        "hang" => calls(vec![call("h", "hang", json!({}))]),
        _ => answer(json!(format!("leaf@{depth}"))),
    }
}

fn session(host: &Arc<Host>, depth: u32, task: &str, tok: CancelToken) -> Result<String, SessionError> {
    host.max_depth_seen.fetch_max(depth as usize, SeqCst);
    let t = |n: &str| ToolSpec { name: n.into(), description: n.into(), params: json!({}) };
    let sp = Spec { tools: vec![t("hang"), t("spawn_agent")], result_schema: json!({"type":"string"}) };
    let validate = |v: &Value| v.is_string().then_some(()).ok_or_else(|| "not a string".to_string());
    let mut model = |req: ModelRequest, _t: &CancelToken| Ok(tree_reply(&req, depth));
    let mut spawned = 0usize;
    let h = host.clone();
    let mut tool = move |c: &ToolCall, t: &CancelToken| -> Result<Value, String> {
        match c.name.as_str() {
            "hang" => {
                let (tx, rx) = mpsc::channel::<()>();
                t.on_cancel(move || {
                    let _ = tx.send(());
                });
                if let Some(s) = h.started.lock().unwrap().take() {
                    let _ = s.send(());
                }
                rx.recv_timeout(Duration::from_secs(5)).map(|_| json!("released")).map_err(|_| "never cancelled".to_string())
            }
            _ => {
                if depth >= h.max_depth {
                    return Err("depth limit reached".into());
                }
                if spawned >= h.max_children {
                    return Err("fan-out limit reached".into());
                }
                spawned += 1;
                let child_tok = CancelToken::new();
                let ct = child_tok.clone();
                t.on_cancel(move || ct.cancel()); // the library's token is how cancel reaches the child
                match session(&h, depth + 1, c.args["task"].as_str().unwrap(), child_tok) {
                    Ok(s) => Ok(json!({ "answer": s })),
                    Err(e) => Err(format!("child failed: {e:?}")),
                }
            }
        }
    };
    let (mut mailbox, mut on_step) = (Vec::new, |_: &State| true);
    let mut w = Wiring { spec: &sp, validate: &validate, cancel: &tok, model: &mut model, tool: &mut tool, mailbox: &mut mailbox, on_step: &mut on_step };
    let (_, r) = run(&mut w, start(SessionId(depth as u64), Config::default(), task));
    let r = r.map(|v| v.as_str().unwrap().to_string());
    host.ledger.lock().unwrap().push((depth, format!("{r:?}")));
    r
}
fn host(max_depth: u32, max_children: usize, started: Option<mpsc::Sender<()>>) -> Arc<Host> {
    Arc::new(Host { max_depth, max_children, ledger: Default::default(), started: Mutex::new(started), max_depth_seen: AtomicUsize::new(0) })
}

#[test]
fn spawn_depth_limit() {
    let h = host(2, 10, None);
    assert_eq!(session(&h, 0, "recurse", CancelToken::new()).unwrap(), "d0[d1[d2[depth limit reached]]]");
    assert_eq!((h.max_depth_seen.load(SeqCst), h.ledger.lock().unwrap().len()), (2, 3));
}

#[test]
fn spawn_fanout_limit() {
    // sequential plain code: the limit is a per-session budget (live children are never
    // concurrent here), not a concurrency cap as in the async owners.
    let h = host(3, 2, None);
    let s = session(&h, 0, "fan", CancelToken::new()).unwrap();
    assert_eq!((s.matches("leaf@1").count(), s.matches("fan-out limit reached").count()), (2, 1), "{s}");
}

#[test]
fn cancel_reaches_spawned_child() {
    let (tx, rx) = mpsc::channel();
    let h = host(3, 3, Some(tx));
    let root = CancelToken::new();
    let (h2, r2) = (h.clone(), root.clone());
    let worker = std::thread::spawn(move || session(&h2, 0, "delegate-hang", r2));
    rx.recv_timeout(Duration::from_secs(5)).unwrap(); // the grandchild-level tool is blocked
    root.cancel(); // hooks run here, synchronously: root's hook -> child token -> hang's hook
    assert_eq!(worker.join().unwrap(), Err(SessionError::Aborted));
    let mut l = h.ledger.lock().unwrap().clone();
    l.sort();
    assert_eq!(l, vec![(0, "Err(Aborted)".to_string()), (1, "Err(Aborted)".to_string())]);
}

// ---- state out / state in, and a budget with a fake clock -------------------------------
#[test]
fn checkpoint_and_resume_in_a_fresh_run() {
    let store: Arc<Mutex<Vec<String>>> = Default::default();
    let s2 = store.clone();
    let (_seen, mut model) = scripted(vec![calls(vec![call("c1", "add", json!({"a": 2, "b": 3}))]), answer(good())], |_| {});
    let (sp, cancel) = (spec(), CancelToken::new());
    let (mut tool, mut mailbox) = (basic_tool, Vec::new);
    let mut on_step = move |s: &State| {
        s2.lock().unwrap().push(serde_json::to_string(s).unwrap());
        true
    };
    let mut w = Wiring { spec: &sp, validate: &verdict_ok, cancel: &cancel, model: &mut model, tool: &mut tool, mailbox: &mut mailbox, on_step: &mut on_step };
    assert_eq!(run(&mut w, start(SessionId(1), Config::default(), "go")).1, Ok(good()));
    let cps = store.lock().unwrap().clone();
    assert_eq!(cps.len(), 3, "before call 1, before the tool batch, before call 2");

    // resume from "stored bytes" only; a model that knows just the last step
    let state: State = serde_json::from_str(&cps[1]).unwrap();
    let (seen, mut model) = scripted(vec![answer(good())], |_| {});
    let (mut mailbox, mut on_step) = (Vec::new, |_: &State| true);
    let mut w = Wiring { spec: &sp, validate: &verdict_ok, cancel: &cancel, model: &mut model, tool: &mut tool, mailbox: &mut mailbox, on_step: &mut on_step };
    assert_eq!(run(&mut w, state).1, Ok(good()));
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[test]
fn a_wall_clock_budget_with_a_fake_clock_and_no_timers() {
    // The core has no clock input at all. A host that wants a time budget reads its own
    // (here: fake) clock in the step hook and stops the run. Deterministic, no sleeping.
    let clock = Arc::new(Mutex::new(0u64)); // fake ms
    let c_model = clock.clone();
    let (_seen, mut model) = scripted((0..10).map(|i| calls(vec![call(&format!("c{i}"), "echo", json!(i))])).collect(), move |_| *c_model.lock().unwrap() += 40);
    let (sp, cancel) = (spec(), CancelToken::new());
    let (mut tool, mut mailbox) = (basic_tool, Vec::new);
    let c_hook = clock.clone();
    let mut on_step = move |_: &State| *c_hook.lock().unwrap() <= 100;
    let mut w = Wiring { spec: &sp, validate: &verdict_ok, cancel: &cancel, model: &mut model, tool: &mut tool, mailbox: &mut mailbox, on_step: &mut on_step };
    let (st, r) = run(&mut w, start(SessionId(1), Config::default(), "go"));
    assert_eq!(r, Err(SessionError::Aborted));
    assert_eq!(*clock.lock().unwrap(), 120, "3 model calls at 40 ms each, then the 100 ms budget stops the run");
    assert_eq!(st.turn, 3);
}

// ---- measurements printed with --nocapture ---------------------------------------------
#[test]
fn owner_has_no_async_and_no_executor() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs")).unwrap();
    for bad in ["async", ".await", "futures", "tokio", "Pin<", "Box<dyn", "spawn"] {
        assert!(!src.lines().filter(|l| !l.trim_start().starts_with("//")).any(|l| l.contains(bad)), "host-owned lib.rs mentions {bad}");
    }
}

// ---- the smallest host loop: typed result + tools, nothing else (no steer, no abort) ------
// MINIMAL-LOOP-START
fn minimal(spec: &Spec, validate: &dyn Fn(&Value) -> Result<(), String>, model: &mut dyn FnMut(ModelRequest) -> Result<ModelReply, String>, tool: &mut dyn FnMut(&ToolCall) -> Result<Value, String>, state: State) -> Result<Value, SessionError> {
    let mut out = resume(state, spec);
    loop {
        out = match out.step {
            Step::Call(req) => absorb(out.state, spec, validate, model(req)),
            Step::Tools(calls) => {
                let done = calls.iter().map(|c| ToolDone { call_id: c.id.clone(), result: tool(c) }).collect();
                tools_done(out.state, spec, done)
            }
            Step::End(r) => return r,
        };
    }
}
// MINIMAL-LOOP-END

#[test]
fn the_minimal_host_loop_runs_the_typed_result_scenario() {
    let (seen, mut m) = scripted(vec![calls(vec![call("c1", "add", json!({"a": 2, "b": 3}))]), answer(good())], |_| {});
    let tok = CancelToken::new();
    let r = minimal(&spec(), &verdict_ok, &mut |req| m(req, &tok), &mut |c| basic_tool(c, &tok), start(SessionId(1), Config::default(), "go"));
    assert_eq!(r, Ok(good()));
    assert_eq!(seen.lock().unwrap().len(), 2);
}
