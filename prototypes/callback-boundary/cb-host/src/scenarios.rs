//! The scenarios of ticket 35 re-expressed over `Rig` (same assertions; the shell they used
//! does not ship after ADR 0004), plus the two new streaming ones. Each takes a `Rig` and runs
//! unchanged under B1, B2 and B3.
#![allow(unused_must_use)]
use crate::host::*;
use crate::mock_net::*;
use crate::rig::*;
use cb_core::*;
use futures::channel::oneshot;
use futures::join;
use serde_json::{json, Value};
use std::sync::atomic::Ordering::SeqCst;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub fn new_state(id: u64, prompt: &str) -> State {
    start(SessionId(id), Config::default(), prompt)
}
fn label(v: Value) -> Verdict {
    serde_json::from_value(v).unwrap()
}
pub async fn wait_until(mut f: impl FnMut() -> bool, ms: u64) -> bool {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        if f() {
            return true;
        }
        delay(Duration::from_millis(1)).await;
    }
    f()
}

/// (1) typed result via `final_answer` plus a tool.
pub async fn typed_result(rig: Rig) {
    let (script, model) = scripted(vec![
        Turn::new(calls(vec![call("c1", "add", json!({"a": 2, "b": 3}))])),
        Turn::new(answer(good())),
    ]);
    let hc = basic_caps(model, Default::default());
    let obs = Obs::default();
    let (st, r) = rig.run(&hc, new_state(1, "go"), RunIo::new(&obs)).await;
    assert_eq!(label(r.unwrap()), Verdict { score: 7, label: "ok".into() });
    assert_eq!(st.turn, 2);
    let seen = script.seen.lock().unwrap();
    assert!(matches!(&seen[1][2], Message::ToolResult { content, .. } if content == "5"));
    assert!(obs.0.lock().unwrap().iter().any(|(_, e)| matches!(e, Ev::Core(Event::ToolEnd { .. }))), "core events reach the observer");
}

/// (2) malformed answers: retry and recover; exhaust; never answer.
pub async fn malformed_results(rig: Rig) {
    let (script, model) = scripted(vec![Turn::new(answer(json!({"score": "high"}))), Turn::new(answer(good()))]);
    let hc = basic_caps(model, Default::default());
    let (_, r) = rig.run(&hc, new_state(1, "go"), RunIo::new(&Obs::default())).await;
    assert!(r.is_ok());
    let seen = script.seen.lock().unwrap();
    let Message::ToolResult { content, is_error, .. } = &seen[1][2] else { panic!() };
    assert!(*is_error && content.starts_with("rejected:"), "{content}");
    drop(seen);

    let (_s, model) = scripted((0..5).map(|_| Turn::new(answer(json!({"nope": 1})))).collect());
    let (_, r) = rig.run(&basic_caps(model, Default::default()), new_state(2, "go"), RunIo::new(&Obs::default())).await;
    match r {
        Err(SessionError::Malformed { attempts, raw, .. }) => assert_eq!((attempts, raw), (3, json!({"nope": 1}))),
        o => panic!("{o:?}"),
    }
    let (_s, model) = scripted(vec![Turn::new(say("hi")), Turn::new(say("still hi"))]);
    let (_, r) = rig.run(&basic_caps(model, Default::default()), new_state(3, "go"), RunIo::new(&Obs::default())).await;
    assert_eq!(r, Err(SessionError::NoAnswer));
}

/// (3) steer lands at the next boundary; follow-up only when the run would stop.
pub async fn steer_follow_up(rig: Rig) {
    let (gate_tx, gate_rx) = oneshot::channel();
    let (started_tx, started_rx) = oneshot::channel();
    let (script, model) = scripted(vec![
        Turn::new(calls(vec![call("c1", "echo", json!("x"))])).held(gate_rx, started_tx),
        Turn::new(answer(json!({"score": 1, "label": "first"}))),
        Turn::new(answer(json!({"score": 2, "label": "after follow-up"}))),
    ]);
    let hc = basic_caps(model, Default::default());
    let io = RunIo::new(&Obs::default());
    let inbox = io.inbox.clone();
    let drv = async move {
        started_rx.await.unwrap(); // model call 1 is in flight
        inbox.steer("steer-1");
        inbox.follow_up("follow-1");
        gate_tx.send(()).unwrap();
    };
    let ((_, r), ()) = join!(rig.run(&hc, new_state(1, "go"), io), drv);
    assert_eq!(label(r.unwrap()).label, "after follow-up");
    let seen = script.seen.lock().unwrap();
    assert_eq!(user_texts(&seen[0]), ["go"]);
    assert_eq!(user_texts(&seen[1]), ["go", "steer-1"]);
    assert_eq!(user_texts(&seen[2]), ["go", "steer-1", "follow-1"]);
}

/// (4) abort while a tool hangs: the run ends Aborted and the hung callback is released.
/// Returns (ms from abort until the host's hung callback was dropped, whether the hook ran).
pub async fn abort_hang(rig: Rig) -> (u128, bool) {
    let probe: Arc<Probe> = Default::default();
    let (st_tx, st_rx) = oneshot::channel();
    *probe.started.lock().unwrap() = Some(st_tx);
    let (_s, model) = scripted(vec![Turn::new(calls(vec![call("c", "hang", json!({}))]))]);
    let hc = basic_caps(model, probe.clone());
    let io = RunIo::new(&Obs::default());
    let tok = io.token.clone();
    let p2 = probe.clone();
    let drv = async move {
        st_rx.await.unwrap();
        let t = Instant::now();
        tok.cancel();
        t
    };
    let ((_, r), t) = join!(rig.run(&hc, new_state(1, "go"), io), drv);
    assert_eq!(r, Err(SessionError::Aborted));
    assert!(wait_until(|| p2.dropped.load(SeqCst), 2000).await, "the hung tool was never released");
    (t.elapsed().as_millis(), probe.saw_cancel.load(SeqCst))
}

/// (5) a spawned child through an ordinary tool. The tool calls the library again.
pub async fn spawn_child(rig: Rig) {
    let r2 = rig.clone();
    let (_s, pmodel) = scripted(vec![Turn::new(calls(vec![call("s", "spawn_agent", json!({"task": "leaf"}))])), Turn::new(answer(json!({"score": 1, "label": "parent"})))]);
    let hc = basic_caps(pmodel, Default::default()).tool("spawn_agent", "run a sub-agent", move |_args, api| {
        let rig = r2.clone();
        Box::pin(async move {
            let (_s, cmodel) = scripted(vec![Turn::new(answer(json!({"score": 2, "label": "child"})))]);
            let io = RunIo { token: link_child(&api), inbox: Inbox::default(), observer: Obs::default().sink() };
            let (_, r) = rig.run(&basic_caps(cmodel, Default::default()), new_state(99, "leaf"), io.clone()).await;
            io.token.cancel(); // lets the polling thread (B2, B3) end
            r.map_err(|e| format!("child failed: {e:?}"))
        })
    });
    let (st, r) = rig.run(&hc, new_state(1, "go"), RunIo::new(&Obs::default())).await;
    assert_eq!(label(r.unwrap()).label, "parent");
    let tr = st.messages.iter().find_map(|m| if let Message::ToolResult { name, content, .. } = m { (name == "spawn_agent").then(|| content.clone()) } else { None }).unwrap();
    assert!(tr.contains("child"), "{tr}");
}

/// (5b) parent aborted while the child hangs, the child living INSIDE the tool's future (the
/// "inline" case of ticket 35, which needs the extra poll). Returns (ms until the child's hung
/// tool was released, whether the child's `run` returned `Aborted` to the tool that started it).
pub async fn cancel_reaches_child(rig: Rig) -> (u128, bool) {
    cancel_child(rig, true).await
}
/// (5c) the same, the child spawned on the host's executor (35's "host-spawned" case).
pub async fn cancel_reaches_spawned_child(rig: Rig) -> (u128, bool) {
    let r = cancel_child(rig, false).await;
    assert!(r.1, "a host-spawned child must still report Aborted");
    r
}
async fn cancel_child(rig: Rig, inline: bool) -> (u128, bool) {
    let reported: Arc<std::sync::atomic::AtomicBool> = Default::default();
    let rep2 = reported.clone();
    let child_probe: Arc<Probe> = Default::default();
    let (cst_tx, cst_rx) = oneshot::channel();
    *child_probe.started.lock().unwrap() = Some(cst_tx);
    let (r2, cp) = (rig.clone(), child_probe.clone());
    let (_s, pmodel) = scripted(vec![Turn::new(calls(vec![call("s", "spawn_agent", json!({}))]))]);
    let hc = basic_caps(pmodel, Default::default()).tool("spawn_agent", "run a sub-agent", move |_args, api| {
        let (rig, cp, rep) = (r2.clone(), cp.clone(), rep2.clone());
        Box::pin(async move {
            let (_s, cmodel) = scripted(vec![Turn::new(calls(vec![call("h", "hang", json!({}))]))]);
            let io = RunIo { token: link_child(&api), inbox: Inbox::default(), observer: Obs::default().sink() };
            let child = {
                let rig = rig.clone();
                async move {
                    let (_, r) = rig.run(&basic_caps(cmodel, cp), new_state(99, "leaf"), io.clone()).await;
                    if r == Err(SessionError::Aborted) {
                        rep.store(true, SeqCst);
                    }
                    io.token.cancel(); // ends the polling thread (B2, B3)
                    r
                }
            };
            let r = if inline {
                child.await
            } else {
                let (tx, rx) = oneshot::channel();
                rig.pool.spawn_ok(async move {
                    let _ = tx.send(child.await);
                });
                rx.await.unwrap_or(Err(SessionError::Aborted))
            };
            r.map(|_| json!("done")).map_err(|e| format!("{e:?}"))
        })
    });
    let io = RunIo::new(&Obs::default());
    let tok = io.token.clone();
    let cp2 = child_probe.clone();
    let drv = async move {
        cst_rx.await.unwrap(); // the grandchild-level tool is hanging
        let t = Instant::now();
        tok.cancel();
        t
    };
    let ((_, r), t) = join!(rig.run(&hc, new_state(1, "go"), io), drv);
    assert_eq!(r, Err(SessionError::Aborted));
    assert!(wait_until(|| cp2.dropped.load(SeqCst), 3000).await, "the child's tool was never released");
    let ms = t.elapsed().as_millis();
    delay(Duration::from_millis(100)).await; // give a child that outlives its parent's tool time to report
    (ms, reported.load(SeqCst))
}

/// (6) two runs at once, no shared state; B finishes while A is still held.
pub async fn parallel_runs(rig: Rig) {
    let (a_gate, a_rx) = oneshot::channel();
    let (a_started, a_started_rx) = oneshot::channel();
    let (_sa, ma) = scripted(vec![Turn::new(answer(json!({"score": 1, "label": "A"}))).held(a_rx, a_started)]);
    let (_sb, mb) = scripted(vec![Turn::new(answer(json!({"score": 2, "label": "B"})))]);
    let (ha, hb) = (basic_caps(ma, Default::default()), basic_caps(mb, Default::default()));
    let a = rig.run(&ha, new_state(10, "a"), RunIo::new(&Obs::default()));
    let rb = rig.clone();
    let drv = async move {
        a_started_rx.await.unwrap();
        let (_, r) = rb.run(&hb, new_state(11, "b"), RunIo::new(&Obs::default())).await; // B runs to the end first
        let b_label = label(r.unwrap()).label;
        a_gate.send(()).unwrap();
        b_label
    };
    let ((_, ra), b_label) = join!(a, drv);
    assert_eq!((label(ra.unwrap()).label, b_label), ("A".to_string(), "B".to_string()));
}

/// (7, new) a model reply streamed with live deltas to a host sink. Returns (ms to the first
/// delta, ms from the first delta to the end of the run, deltas seen).
pub async fn stream_deltas(rig: Rig) -> (u128, u128, usize) {
    let words = ["alpha ", "beta ", "gamma ", "delta ", "epsilon ", "zeta "];
    let (_s, model) = scripted(vec![Turn::new(answer(good())).streaming(&words, 20)]);
    let hc = basic_caps(model, Default::default());
    let obs = Obs::default();
    let t0 = Instant::now();
    let (_, r) = rig.run(&hc, new_state(1, "go"), RunIo::new(&obs)).await;
    let end = Instant::now();
    assert!(r.is_ok());
    let d = obs.deltas();
    assert_eq!(d.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>(), words, "in order, none lost");
    assert!(end.duration_since(d[0].0) >= Duration::from_millis(80), "the first delta arrived while the call was still running");
    (d[0].0.duration_since(t0).as_millis(), end.duration_since(d[0].0).as_millis(), d.len())
}

fn net_caps() -> HostCaps {
    let (_s, model) = scripted(vec![]);
    let mut hc = basic_caps(model, Default::default());
    hc.transport = Some(Arc::new(|req, _api| Box::pin(async move { connect(&req) })));
    hc
}

/// (8, new) a Transport call with a streamed byte body over a loopback mock.
pub async fn transport_stream(rig: Rig) -> (u128, usize) {
    let srv = serve(word_chunks(8, 20));
    let obs = Obs::default();
    let t0 = Instant::now();
    let r = rig.exchange(&net_caps(), HttpReq { url: srv.url.clone(), body: "{}".into() }, RunIo::new(&obs)).await;
    let end = Instant::now();
    let reply = r.expect("not cancelled").unwrap();
    assert_eq!(reply.text, (0..8).map(|i| format!("w{i} ")).collect::<String>());
    let d = obs.deltas();
    assert_eq!(d.len(), 8);
    assert!(end.duration_since(d[0].0) >= Duration::from_millis(60), "deltas arrived live, not at the end");
    (d[0].0.duration_since(t0).as_millis(), d.len())
}

/// (8b, new) dropping the body cancels the call: returns ms from cancel until the mock server saw the hang-up.
pub async fn transport_cancel(rig: Rig) -> (u128, u64) {
    let srv = serve(word_chunks(60, 20));
    let obs = Obs::default();
    let io = RunIo::new(&obs);
    let (tok, o2) = (io.token.clone(), obs.clone());
    let drv = async move {
        assert!(wait_until(|| o2.deltas().len() >= 3, 3000).await);
        let t = Instant::now();
        tok.cancel();
        t
    };
    let hc = net_caps();
    let (r, t) = join!(rig.exchange(&hc, HttpReq { url: srv.url.clone(), body: "{}".into() }, io), drv);
    assert!(r.is_none(), "cancelled, not completed");
    let log = srv.log.clone();
    assert!(wait_until(|| log.client_closed_at.lock().unwrap().is_some(), 2000).await, "the server never saw the hang-up");
    let closed = log.client_closed_at.lock().unwrap().unwrap();
    assert!(!*log.finished.lock().unwrap());
    (closed.saturating_duration_since(t).as_millis(), log.chunks_sent.load(SeqCst))
}
