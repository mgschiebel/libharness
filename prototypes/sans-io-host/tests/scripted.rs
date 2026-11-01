//! Deterministic tests: scripted fake model, fake clock, no threads, no sleeping.
use sans_io_host_proto::*;
use std::time::{Duration, Instant};

struct H {
    a: Agent,
    t0: Instant,
    ms: u64,
}
impl H {
    fn new(cfg: Config) -> Self {
        H { a: Agent::new(cfg), t0: Instant::now(), ms: 0 }
    }
    fn now(&self) -> Instant {
        self.t0 + Duration::from_millis(self.ms)
    }
    fn tick(&mut self, ms: u64) -> Instant {
        self.ms += ms;
        self.now()
    }
    fn drain(&mut self) -> Vec<Output> {
        std::iter::from_fn(|| self.a.poll_output()).collect()
    }
    fn events(outs: &[Output]) -> Vec<AgentEvent> {
        outs.iter().filter_map(|o| if let Output::Event(e) = o { Some(e.clone()) } else { None }).collect()
    }
    fn model_call(outs: &[Output]) -> Option<ModelRequest> {
        outs.iter().find_map(|o| if let Output::CallModel(r) = o { Some(r.clone()) } else { None })
    }
    fn say(&mut self, call: CallId, evs: Vec<ModelEvent>) {
        for e in evs {
            let n = self.tick(5);
            self.a.handle_model_event(n, call, e);
        }
    }
}
fn tc(id: &str) -> ToolCall {
    ToolCall { id: id.into(), name: "t".into(), args: "".into() }
}
fn ok(s: &str) -> ToolResult {
    ToolResult { content: s.into(), is_error: false }
}
fn tool_call_turn(h: &mut H, call: CallId, ids: &[&str]) {
    let mut evs: Vec<_> = ids.iter().map(|i| ModelEvent::ToolCall(tc(i))).collect();
    evs.push(ModelEvent::Done(StopReason::ToolUse));
    h.say(call, evs);
}

#[test]
fn prompt_streams_deltas_then_ends() {
    let mut h = H::new(Config::default());
    let n = h.now();
    h.a.prompt(n, "hi").unwrap();
    let outs = h.drain();
    let req = H::model_call(&outs).unwrap();
    assert_eq!(req.messages, vec![Message::User("hi".into())]);
    h.say(req.call_id, vec![ModelEvent::TextDelta("he".into()), ModelEvent::TextDelta("llo".into()), ModelEvent::Done(StopReason::EndTurn)]);
    let ev = H::events(&h.drain());
    assert!(matches!(ev.first(), Some(AgentEvent::MessageStart { role: "assistant" })));
    assert_eq!(ev.iter().filter(|e| matches!(e, AgentEvent::MessageUpdate { .. })).count(), 2);
    assert!(matches!(ev.last(), Some(AgentEvent::AgentEnd { aborted: false })));
    assert!(h.a.is_idle());
    assert!(h.a.prompt(h.now(), "again").is_ok());
}

#[test]
fn prompt_while_running_is_busy() {
    let mut h = H::new(Config::default());
    h.a.prompt(h.now(), "x").unwrap();
    assert_eq!(h.a.prompt(h.now(), "y"), Err(Busy));
}

#[test]
fn parallel_tools_emitted_together_results_ordered_by_source() {
    let mut h = H::new(Config::default());
    h.a.prompt(h.now(), "go").unwrap();
    let c = H::model_call(&h.drain()).unwrap().call_id;
    tool_call_turn(&mut h, c, &["a", "b"]);
    let outs = h.drain();
    let runs: Vec<_> = outs.iter().filter(|o| matches!(o, Output::RunTool(_))).collect();
    assert_eq!(runs.len(), 2, "both tools dispatched before any result");
    // b finishes first
    let n = h.tick(10);
    h.a.handle_tool_result(n, "b", ok("B"));
    let n = h.tick(10);
    h.a.handle_tool_result(n, "a", ok("A"));
    let outs = h.drain();
    let ends: Vec<_> = H::events(&outs).into_iter().filter_map(|e| if let AgentEvent::ToolExecutionEnd { id, .. } = e { Some(id) } else { None }).collect();
    assert_eq!(ends, vec!["b", "a"], "execution_end in completion order");
    let req = H::model_call(&outs).unwrap();
    let ids: Vec<_> = req.messages.iter().filter_map(|m| if let Message::ToolResult { tool_call_id, .. } = m { Some(tool_call_id.as_str()) } else { None }).collect();
    assert_eq!(ids, vec!["a", "b"], "tool-result messages in source order");
}

#[test]
fn steering_waits_for_whole_batch_then_precedes_next_model_call() {
    let mut h = H::new(Config::default());
    h.a.prompt(h.now(), "go").unwrap();
    let c = H::model_call(&h.drain()).unwrap().call_id;
    tool_call_turn(&mut h, c, &["a", "b"]);
    h.drain();
    h.a.steer(h.now(), "actually do X");
    let n = h.tick(1);
    h.a.handle_tool_result(n, "a", ok("A"));
    assert!(H::model_call(&h.drain()).is_none(), "no model call while batch incomplete");
    let n = h.tick(1);
    h.a.handle_tool_result(n, "b", ok("B"));
    let req = H::model_call(&h.drain()).unwrap();
    assert_eq!(req.messages.last(), Some(&Message::User("actually do X".into())));
    assert_eq!(req.messages.iter().filter(|m| matches!(m, Message::ToolResult { .. })).count(), 2, "tools were not skipped");
}

#[test]
fn follow_up_only_when_agent_would_stop() {
    let mut h = H::new(Config::default());
    h.a.prompt(h.now(), "go").unwrap();
    let c = H::model_call(&h.drain()).unwrap().call_id;
    h.a.follow_up(h.now(), "then this");
    h.drain();
    tool_call_turn(&mut h, c, &["a"]);
    h.drain();
    let n = h.tick(1);
    h.a.handle_tool_result(n, "a", ok("A"));
    let outs = h.drain();
    let req = H::model_call(&outs).unwrap();
    assert_ne!(req.messages.last(), Some(&Message::User("then this".into())), "tool turn continues first");
    h.say(req.call_id, vec![ModelEvent::TextDelta("done".into()), ModelEvent::Done(StopReason::EndTurn)]);
    let outs = h.drain();
    let req2 = H::model_call(&outs).expect("follow-up starts a new turn");
    assert_eq!(req2.messages.last(), Some(&Message::User("then this".into())));
    assert!(!H::events(&outs).iter().any(|e| matches!(e, AgentEvent::AgentEnd { .. })));
}

#[test]
fn abort_cancels_pending_tools_keeps_history_valid_and_ignores_late_results() {
    let mut h = H::new(Config::default());
    h.a.prompt(h.now(), "go").unwrap();
    let c = H::model_call(&h.drain()).unwrap().call_id;
    tool_call_turn(&mut h, c, &["a", "b"]);
    h.drain();
    let n = h.tick(1);
    h.a.handle_tool_result(n, "a", ok("A"));
    h.drain();
    h.a.steer(h.now(), "queued");
    h.drain();
    let n = h.tick(1);
    h.a.abort(n);
    let outs = h.drain();
    assert!(outs.contains(&Output::CancelTool { tool_call_id: "b".into() }));
    assert!(!outs.contains(&Output::CancelTool { tool_call_id: "a".into() }));
    assert!(H::events(&outs).contains(&AgentEvent::AgentEnd { aborted: true }));
    assert!(h.a.is_idle());
    let results = h.a.messages().iter().filter(|m| matches!(m, Message::ToolResult { .. })).count();
    assert_eq!(results, 2, "every tool call has a result");
    let n = h.tick(1);
    h.a.handle_tool_result(n, "b", ok("late"));
    h.a.handle_model_event(n, c, ModelEvent::TextDelta("late".into()));
    assert!(h.drain().is_empty(), "late inputs ignored");
    assert_eq!(h.a.take_queued().0, vec!["queued".to_string()]);
}

#[test]
fn abort_during_model_cancels_model_and_keeps_partial_text() {
    let mut h = H::new(Config::default());
    h.a.prompt(h.now(), "go").unwrap();
    let c = H::model_call(&h.drain()).unwrap().call_id;
    h.say(c, vec![ModelEvent::TextDelta("par".into())]);
    h.a.abort(h.now());
    let outs = h.drain();
    assert!(outs.contains(&Output::CancelModel { call_id: c }));
    assert!(matches!(h.a.messages().last(), Some(Message::Assistant { stop: StopReason::Aborted, text, .. }) if text == "par"));
}

#[test]
fn tool_timeout_driven_by_fake_clock() {
    let mut h = H::new(Config { tool_timeout: Some(Duration::from_millis(100)) });
    h.a.prompt(h.now(), "go").unwrap();
    let c = H::model_call(&h.drain()).unwrap().call_id;
    let before = h.now();
    tool_call_turn(&mut h, c, &["a"]);
    h.drain();
    let dl = h.a.poll_timeout().expect("deadline exposed");
    assert!(dl > before);
    h.a.handle_timeout(h.now()); // spurious: no-op
    assert!(h.drain().is_empty());
    let n = h.tick(200);
    h.a.handle_timeout(n);
    let outs = h.drain();
    assert!(outs.contains(&Output::CancelTool { tool_call_id: "a".into() }));
    let req = H::model_call(&outs).unwrap();
    assert!(matches!(req.messages.last(), Some(Message::ToolResult { is_error: true, .. })));
    assert_eq!(h.a.poll_timeout(), None);
}

#[test]
fn length_stop_fails_tool_calls_instead_of_running() {
    let mut h = H::new(Config::default());
    h.a.prompt(h.now(), "go").unwrap();
    let c = H::model_call(&h.drain()).unwrap().call_id;
    h.say(c, vec![ModelEvent::ToolCall(tc("a")), ModelEvent::Done(StopReason::Length)]);
    let outs = h.drain();
    assert!(!outs.iter().any(|o| matches!(o, Output::RunTool(_))));
    assert!(matches!(H::model_call(&outs).unwrap().messages.last(), Some(Message::ToolResult { is_error: true, .. })));
}

#[test]
fn model_failure_ends_run() {
    let mut h = H::new(Config::default());
    h.a.prompt(h.now(), "go").unwrap();
    let c = H::model_call(&h.drain()).unwrap().call_id;
    h.say(c, vec![ModelEvent::Failed("500".into())]);
    assert!(H::events(&h.drain()).contains(&AgentEvent::AgentEnd { aborted: false }));
    assert!(h.a.is_idle());
}
