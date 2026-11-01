//! Extra behaviours the nine scenarios do not pin down. Same file runs against
//! session-task (host-tests) and session-core (host-tests-core). tokio only.
use futures::channel::{mpsc, oneshot};
use futures::StreamExt;
use host_tests::glue::*;
use serde_json::json;
use session_task::*;
use std::sync::Arc;

fn sp() -> Spawner {
    Arc::new(|f| {
        tokio::spawn(f);
    })
}
fn users(m: &[Message]) -> Vec<String> {
    m.iter().filter_map(|m| if let Message::User(s) = m { Some(s.clone()) } else { None }).collect()
}

/// Two steers queued during one model call. Does each engine inject both at once?
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_steer() {
    let (rtx, mut rrx) = mpsc::unbounded();
    let (gate, gate_rx) = oneshot::channel();
    let (started, started_rx) = oneshot::channel();
    let (script, model) = scripted(vec![
        Step::new(calls(vec![call("c1", "echo", json!("x"))])).held(gate_rx, started),
        Step::new(answer(json!({"score": 1, "label": "a"}))),
        Step::new(answer(json!({"score": 2, "label": "b"}))),
    ]);
    let (h, _end) = launch(&sp(), basic_caps(model, rtx, Default::default()), SessionConfig::new(1), "go");
    started_rx.await.unwrap();
    h.steer("s1");
    h.steer("s2");
    gate.send(()).unwrap();
    assert!(rrx.next().await.unwrap().1.is_ok());
    let seen = script.seen.lock().unwrap();
    let per_request: Vec<Vec<String>> = seen.iter().map(|m| users(m)).collect();
    eprintln!("MULTI_STEER requests={} user texts per request={per_request:?}", seen.len());
    assert!(users(seen.last().unwrap()).ends_with(&["s1".to_string(), "s2".to_string()]));
}

/// max_turns bounds the loop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn max_turns() {
    let (rtx, mut rrx) = mpsc::unbounded();
    let (script, model) = scripted((0..10).map(|i| Step::new(calls(vec![call(&format!("c{i}"), "echo", json!(i))]))).collect());
    let mut cfg = SessionConfig::new(1);
    cfg.max_turns = 3;
    let (_h, end) = launch(&sp(), basic_caps(model, rtx, Default::default()), cfg, "go");
    assert_eq!(rrx.next().await.unwrap().1, Err(SessionError::MaxTurns));
    assert_eq!(end.await.unwrap(), SessionEnd::Failed);
    assert_eq!(script.seen.lock().unwrap().len(), 3);
}

/// Unknown tool is an error result the model sees; a parallel batch is returned in source order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_tool_and_batch_order() {
    let (rtx, mut rrx) = mpsc::unbounded();
    let (script, model) = scripted(vec![
        Step::new(calls(vec![call("a", "nope", json!({})), call("b", "add", json!({"a": 1, "b": 2})), call("c", "echo", json!("z"))])),
        Step::new(answer(json!({"score": 1, "label": "a"}))),
    ]);
    let (_h, _end) = launch(&sp(), basic_caps(model, rtx, Default::default()), SessionConfig::new(1), "go");
    assert!(rrx.next().await.unwrap().1.is_ok());
    let seen = script.seen.lock().unwrap();
    let results: Vec<(String, String, String, bool)> = seen[1]
        .iter()
        .filter_map(|m| if let Message::ToolResult { call_id, name, content, is_error } = m { Some((call_id.clone(), name.clone(), content.clone(), *is_error)) } else { None })
        .collect();
    assert_eq!(
        results,
        vec![
            ("a".into(), "nope".into(), "unknown tool nope".into(), true),
            ("b".into(), "add".into(), "3".into(), false),
            ("c".into(), "echo".into(), "\"z\"".into(), false),
        ]
    );
}

/// Steer sent before the future is first polled (handle returned by Session::start, nothing spawned yet).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_before_first_poll() {
    let (rtx, mut rrx) = mpsc::unbounded();
    let (script, model) = scripted(vec![
        Step::new(calls(vec![call("c1", "echo", json!("x"))])),
        Step::new(answer(json!({"score": 1, "label": "a"}))),
    ]);
    let (h, fut) = Session::start(basic_caps(model, rtx, Default::default()), SessionConfig::new(1), "go");
    assert!(h.steer("early"));
    tokio::spawn(fut);
    assert!(rrx.next().await.unwrap().1.is_ok());
    let seen = script.seen.lock().unwrap();
    let per_request: Vec<Vec<String>> = seen.iter().map(|m| users(m)).collect();
    eprintln!("EARLY_STEER user texts per request={per_request:?}");
    assert!(users(seen.last().unwrap()).contains(&"early".to_string()), "never delivered");
}
