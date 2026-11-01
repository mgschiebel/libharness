//! Pure-logic tests for the session policy: no executor, no futures crate, no threads,
//! fake clock. Each of these needed a runtime plus scripted async fakes in session-task.
use serde::Deserialize;
use serde_json::{json, Value};
use session_core::*;
use std::time::{Duration, Instant};

#[derive(Deserialize, Debug, PartialEq)]
struct Verdict {
    score: u32,
}

struct T {
    e: Engine<Verdict>,
    t0: Instant,
    ms: u64,
}
impl T {
    fn new(cfg: SessionConfig, timeout_ms: Option<u64>) -> Self {
        let t0 = Instant::now();
        T { e: Engine::new(cfg, json!({"type": "object"}), timeout_ms.map(Duration::from_millis), t0), t0, ms: 0 }
    }
    fn now(&mut self) -> Instant {
        self.ms += 1;
        self.t0 + Duration::from_millis(self.ms)
    }
    fn at(&self, ms: u64) -> Instant {
        self.t0 + Duration::from_millis(ms)
    }
    fn start(&mut self, p: &str) -> Vec<Action<Verdict>> {
        let n = self.now();
        self.e.start(n, p.into());
        self.drain()
    }
    fn drain(&mut self) -> Vec<Action<Verdict>> {
        std::iter::from_fn(|| self.e.poll()).collect()
    }
    fn model(&mut self, id: u64, r: Result<ModelReply, String>) -> Vec<Action<Verdict>> {
        let n = self.now();
        self.e.model_done(n, id, r);
        self.drain()
    }
    fn tool(&mut self, id: &str, r: Result<Value, String>) -> Vec<Action<Verdict>> {
        let n = self.now();
        self.e.tool_done(n, id, r);
        self.drain()
    }
}
fn cfg() -> SessionConfig {
    SessionConfig::new(1)
}
fn tc(id: &str, name: &str, args: Value) -> ToolCall {
    ToolCall { id: id.into(), name: name.into(), args }
}
fn reply(calls: Vec<ToolCall>) -> Result<ModelReply, String> {
    Ok(ModelReply { text: String::new(), tool_calls: calls, tokens: 1 })
}
fn answer(v: Value) -> Result<ModelReply, String> {
    reply(vec![tc("fa", FINAL_ANSWER, json!({"result": v}))])
}
fn good() -> Value {
    json!({"score": 7})
}
fn say(t: &str) -> Result<ModelReply, String> {
    Ok(ModelReply { text: t.into(), tool_calls: vec![], tokens: 1 })
}
fn model_call(a: &[Action<Verdict>]) -> Option<(u64, Vec<Message>)> {
    a.iter().find_map(|x| if let Action::CallModel { call_id, messages } = x { Some((*call_id, messages.clone())) } else { None })
}
fn run_tools(a: &[Action<Verdict>]) -> Vec<String> {
    a.iter().filter_map(|x| if let Action::RunTool { id, .. } = x { Some(id.clone()) } else { None }).collect()
}
fn finish(a: Vec<Action<Verdict>>) -> Option<Result<Verdict, SessionError>> {
    a.into_iter().find_map(|x| if let Action::Finish(r) = x { Some(r) } else { None })
}
fn users(m: &[Message]) -> Vec<String> {
    m.iter().filter_map(|m| if let Message::User(s) = m { Some(s.clone()) } else { None }).collect()
}
fn tool_results(m: &[Message]) -> Vec<(String, String, String, bool)> {
    m.iter()
        .filter_map(|m| if let Message::ToolResult { call_id, name, content, is_error } = m { Some((call_id.clone(), name.clone(), content.clone(), *is_error)) } else { None })
        .collect()
}

#[test]
fn typed_result_after_a_tool() {
    let mut t = T::new(cfg(), None);
    let (c1, _) = model_call(&t.start("go")).unwrap();
    let a = t.model(c1, reply(vec![tc("t1", "add", json!({"a": 2, "b": 3}))]));
    assert_eq!(run_tools(&a), ["t1"]);
    let a = t.tool("t1", Ok(json!(5)));
    let (c2, msgs) = model_call(&a).unwrap();
    assert_eq!(tool_results(&msgs), [("t1".into(), "add".into(), "5".into(), false)]);
    let a = t.model(c2, answer(good()));
    assert!(run_tools(&a).is_empty(), "final_answer never reaches the driver");
    assert_eq!(finish(a), Some(Ok(Verdict { score: 7 })));
    assert!(t.e.poll().is_none());
}

#[test]
fn malformed_answer_is_fed_back_then_bounded() {
    let mut t = T::new(cfg(), None);
    let (mut c, _) = model_call(&t.start("go")).unwrap();
    for attempt in 1..=2 {
        let a = t.model(c, answer(json!({"score": "high"})));
        let (next, msgs) = model_call(&a).expect("retry");
        let (_, name, content, err) = tool_results(&msgs).pop().unwrap();
        assert!(name == FINAL_ANSWER && err && content.starts_with("rejected:") && content.contains("Expected schema"), "attempt {attempt}: {content}");
        c = next;
    }
    match finish(t.model(c, answer(json!({"nope": 1})))) {
        Some(Err(SessionError::Malformed { attempts: 3, raw, .. })) => assert_eq!(raw, json!({"nope": 1})),
        other => panic!("{other:?}"),
    }
}

#[test]
fn malformed_then_good_recovers() {
    let mut t = T::new(cfg(), None);
    let (c, _) = model_call(&t.start("go")).unwrap();
    let (c, _) = model_call(&t.model(c, answer(json!({"score": -1})))).unwrap();
    assert_eq!(finish(t.model(c, answer(good()))), Some(Ok(Verdict { score: 7 })));
}

#[test]
fn chatty_model_gets_one_nudge_then_no_answer() {
    let mut t = T::new(cfg(), None);
    let (c, _) = model_call(&t.start("go")).unwrap();
    let (c, msgs) = model_call(&t.model(c, say("hi"))).expect("nudged");
    assert_eq!(users(&msgs), ["go", "Finish by calling final_answer."]);
    assert_eq!(finish(t.model(c, say("still hi"))), Some(Err(SessionError::NoAnswer)));
}

#[test]
fn steer_waits_for_turn_boundary_and_follow_up_for_the_stop() {
    let mut t = T::new(cfg(), None);
    let (c1, _) = model_call(&t.start("go")).unwrap();
    let n = t.now();
    t.e.steer(n, "steer-1".into()); // arrives while model call 1 is in flight
    t.e.follow_up(n, "follow-1".into());
    assert!(t.drain().iter().all(|a| !matches!(a, Action::CallModel { .. })), "nothing injected mid-call");
    let a = t.model(c1, reply(vec![tc("e", "echo", json!("x"))]));
    assert_eq!(run_tools(&a), ["e"]);
    let (c2, m2) = model_call(&t.tool("e", Ok(json!("x")))).unwrap();
    assert_eq!(users(&m2), ["go", "steer-1"]);
    let (c3, m3) = model_call(&t.model(c2, answer(json!({"score": 1})))).expect("follow-up keeps it going");
    assert_eq!(users(&m3), ["go", "steer-1", "follow-1"]);
    assert_eq!(finish(t.model(c3, answer(json!({"score": 2})))), Some(Ok(Verdict { score: 2 })));
}

/// Divergence from session-task (which injects all queued steers together): the core pops one per boundary.
#[test]
fn two_steers_arrive_one_per_boundary() {
    let mut t = T::new(cfg(), None);
    let (c1, _) = model_call(&t.start("go")).unwrap();
    let n = t.now();
    t.e.steer(n, "s1".into());
    t.e.steer(n, "s2".into());
    t.drain();
    t.model(c1, reply(vec![tc("e", "echo", json!(1))]));
    let (c2, m2) = model_call(&t.tool("e", Ok(json!(1)))).unwrap();
    assert_eq!(users(&m2), ["go", "s1"]);
    let (_, m3) = model_call(&t.model(c2, answer(good()))).unwrap();
    assert_eq!(users(&m3), ["go", "s1", "s2"]);
}

#[test]
fn abort_mid_tool_finishes_once_and_ignores_stale_input() {
    let mut t = T::new(cfg(), None);
    let (c1, _) = model_call(&t.start("go")).unwrap();
    t.model(c1, reply(vec![tc("h", "hang", json!({}))]));
    let n = t.now();
    t.e.abort(n);
    assert_eq!(finish(t.drain()), Some(Err(SessionError::Aborted)));
    assert!(t.tool("h", Ok(json!("late"))).is_empty());
    assert!(t.model(c1, answer(good())).is_empty());
    let n = t.now();
    t.e.abort(n); // idempotent
    assert!(t.drain().is_empty());
}

#[test]
fn abort_during_model_call() {
    let mut t = T::new(cfg(), None);
    let (c1, _) = model_call(&t.start("go")).unwrap();
    let n = t.now();
    t.e.abort(n);
    assert_eq!(finish(t.drain()), Some(Err(SessionError::Aborted)));
    assert!(t.model(c1, answer(good())).is_empty());
}

#[test]
fn max_turns_bounds_the_run() {
    let mut c = cfg();
    c.max_turns = 2;
    let mut t = T::new(c, None);
    let (c1, _) = model_call(&t.start("go")).unwrap();
    t.model(c1, reply(vec![tc("a", "echo", json!(1))]));
    let (c2, _) = model_call(&t.tool("a", Ok(json!(1)))).unwrap();
    t.model(c2, reply(vec![tc("b", "echo", json!(2))]));
    assert_eq!(finish(t.tool("b", Ok(json!(2)))), Some(Err(SessionError::MaxTurns)));
}

#[test]
fn model_failure_is_a_typed_error() {
    let mut t = T::new(cfg(), None);
    let (c1, _) = model_call(&t.start("go")).unwrap();
    assert_eq!(finish(t.model(c1, Err("boom".into()))), Some(Err(SessionError::ModelFailed("boom".into()))));
}

#[test]
fn parallel_results_land_in_source_order_whatever_completes_first() {
    let mut t = T::new(cfg(), None);
    let (c1, _) = model_call(&t.start("go")).unwrap();
    let a = t.model(c1, reply(vec![tc("a", "x", json!(0)), tc("b", "y", json!(0)), tc("c", "unknown", json!(0))]));
    assert_eq!(run_tools(&a), ["a", "b", "c"]);
    let quiet = |a: Vec<Action<Verdict>>| a.iter().all(|x| matches!(x, Action::Event(_)));
    assert!(quiet(t.tool("c", Err("unknown tool unknown".into()))));
    assert!(quiet(t.tool("a", Ok(json!("A")))));
    let (_, m) = model_call(&t.tool("b", Ok(json!("B")))).unwrap();
    let ids: Vec<_> = tool_results(&m).into_iter().map(|r| r.0).collect();
    assert_eq!(ids, ["a", "b", "c"]);
}

/// The core's explicit clock still works through the engine: a tool deadline expires with
/// no sleeping. (The async driver passes None: it has no timer capability.)
#[test]
fn tool_timeout_with_a_fake_clock() {
    let mut t = T::new(cfg(), Some(50));
    let (c1, _) = model_call(&t.start("go")).unwrap();
    t.model(c1, reply(vec![tc("slow", "x", json!(0))]));
    let deadline = t.e.next_deadline().expect("deadline armed");
    let n = t.at(10);
    t.e.timeout(n); // spurious: no-op
    assert!(t.drain().is_empty());
    t.e.timeout(deadline);
    let (_, m) = model_call(&t.drain()).expect("timeout became an error result and the next turn");
    assert_eq!(tool_results(&m), [("slow".into(), "x".into(), "tool timed out".into(), true)]);
    assert!(t.e.next_deadline().is_none());
}

#[test]
fn events_hide_final_answer_and_report_tokens_and_injections() {
    let mut t = T::new(cfg(), None);
    let mut evs = vec![];
    let mut take = |a: Vec<Action<Verdict>>| {
        a.into_iter().for_each(|x| if let Action::Event(e) = x { evs.push(format!("{e:?}")) })
    };
    let a = t.start("go");
    let (c1, _) = model_call(&a).unwrap();
    take(a);
    let a = t.model(c1, reply(vec![tc("a", "echo", json!(1))]));
    take(a);
    let a = t.tool("a", Ok(json!(1)));
    let (c2, _) = model_call(&a).unwrap();
    take(a);
    take(t.model(c2, answer(good())));
    let joined = evs.join("\n");
    assert!(joined.contains("ToolStart") && joined.contains("ToolEnd") && joined.contains("Tokens"));
    assert!(!joined.contains("final_answer"), "{joined}");
    assert_eq!(joined.matches("TurnStart").count(), 2);
}
