//! The core with no executor, no futures crate, no threads, no callbacks, no clock:
//! plain `#[test]`s that script inputs and read outputs.
use call_core::*;
use serde_json::{json, Value};

fn spec() -> Spec {
    let t = |n: &str| ToolSpec { name: n.into(), description: n.into(), params: json!({}) };
    Spec { tools: vec![t("add"), t("echo")], result_schema: json!({"type":"object"}) }
}
fn validate(v: &Value) -> Result<(), String> {
    (v["score"].is_u64() && v["label"].is_string()).then_some(()).ok_or_else(|| "need score and label".to_string())
}
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
fn done(id: &str, v: Value) -> ToolDone {
    ToolDone { call_id: id.into(), result: Ok(v) }
}
fn new() -> Out {
    begin(start(SessionId(1), Config::default(), "go"), &spec())
}
fn model(o: Out, r: ModelReply) -> Out {
    assert!(matches!(o.step, Step::Call(_)), "expected a model call, got {:?}", o.step);
    absorb(o.state, &spec(), &validate, Ok(r))
}
fn tools(o: Out, r: Vec<ToolDone>) -> Out {
    assert!(matches!(o.step, Step::Tools(_)), "expected tools, got {:?}", o.step);
    tools_done(o.state, &spec(), r)
}
fn req(o: &Out) -> &ModelRequest {
    match &o.step {
        Step::Call(r) => r,
        s => panic!("expected a call, got {s:?}"),
    }
}
fn users(o: &Out) -> Vec<String> {
    req(o).messages.iter().filter_map(|m| if let Message::User(s) = m { Some(s.clone()) } else { None }).collect()
}
fn ended(o: &Out) -> &Result<Value, SessionError> {
    match &o.step {
        Step::End(r) => r,
        s => panic!("expected the end, got {s:?}"),
    }
}

#[test]
fn first_call_carries_the_prompt_and_the_final_answer_tool() {
    let o = new();
    let r = req(&o);
    assert_eq!(users(&o), ["go"]);
    assert_eq!(r.tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["add", "echo", "final_answer"]);
    assert_eq!(o.state.phase, Phase::AwaitingModel);
}

#[test]
fn typed_result_after_a_tool() {
    let o = model(new(), calls(vec![call("c1", "add", json!({"a": 2, "b": 3}))]));
    assert!(matches!(&o.step, Step::Tools(c) if c.len() == 1 && c[0].name == "add"));
    let o = tools(o, vec![done("c1", json!(5))]);
    assert!(matches!(&req(&o).messages[2], Message::ToolResult { content, .. } if content == "5"));
    let o = model(o, answer(good()));
    assert_eq!(ended(&o), &Ok(good())); // answer ends the run: no further model call
    assert_eq!(o.state.phase, Phase::Done);
}

#[test]
fn malformed_answer_is_fed_back_then_recovers() {
    let o = model(new(), answer(json!({"score": "high"})));
    let Message::ToolResult { content, is_error, .. } = &req(&o).messages[2] else { panic!() };
    assert!(*is_error && content.starts_with("rejected:") && content.contains("Expected schema"));
    assert!(o.events.contains(&Event::ResultRejected { session: SessionId(1), error: "need score and label".into() }));
    assert_eq!(ended(&model(o, answer(good()))), &Ok(good()));
}

#[test]
fn malformed_answer_exhausts_retries() {
    let mut o = new();
    for _ in 0..2 {
        o = model(o, answer(json!({"nope": 1})));
        assert!(matches!(o.step, Step::Call(_)));
    }
    let o = model(o, answer(json!({"nope": 1})));
    match ended(&o) {
        Err(SessionError::Malformed { attempts, raw, .. }) => assert_eq!((*attempts, raw), (3, &json!({"nope": 1}))),
        r => panic!("{r:?}"),
    }
}

#[test]
fn chatty_model_gets_one_nudge_then_no_answer() {
    let o = model(new(), say("hi"));
    assert_eq!(users(&o), ["go", "Finish by calling final_answer."]);
    assert_eq!(ended(&model(o, say("still hi"))), &Err(SessionError::NoAnswer));
}

#[test]
fn steer_lands_at_the_next_boundary_and_all_steers_together() {
    let o = new();
    let st = steer(steer(o.state, "s1"), "s2"); // arrived while call 1 was in flight
    let o = absorb(st, &spec(), &validate, Ok(calls(vec![call("c1", "echo", json!("x"))])));
    let o = tools(o, vec![done("c1", json!("x"))]);
    assert_eq!(users(&o), ["go", "s1", "s2"]);
    assert_eq!(o.events.iter().filter(|e| matches!(e, Event::Injected { kind: Inject::Steer, .. })).count(), 2);
}

#[test]
fn steer_before_the_first_call_is_in_request_one() {
    let st = steer(start(SessionId(1), Config::default(), "go"), "early");
    assert_eq!(users(&begin(st, &spec())), ["go", "early"]);
}

#[test]
fn follow_up_is_used_only_when_the_run_would_stop() {
    let o = new();
    let st = follow_up(steer(o.state, "s"), "f");
    let o = absorb(st, &spec(), &validate, Ok(calls(vec![call("c1", "echo", json!(1))])));
    let o = tools(o, vec![done("c1", json!(1))]);
    assert_eq!(users(&o), ["go", "s"]); // follow-up not yet
    let o = model(o, answer(json!({"score": 1, "label": "first"})));
    assert_eq!(users(&o), ["go", "s", "f"]); // accepted answer + queued follow-up: keep going
    let o = model(o, answer(json!({"score": 2, "label": "after follow-up"})));
    assert_eq!(ended(&o).as_ref().unwrap()["label"], "after follow-up");
}

#[test]
fn queued_steer_keeps_the_run_going_after_an_accepted_answer() {
    let o = new();
    let o = absorb(steer(o.state, "late"), &spec(), &validate, Ok(answer(good())));
    assert_eq!(users(&o), ["go", "late"]);
}

#[test]
fn abort_from_every_phase_is_terminal_and_idempotent() {
    let a = abort(new().state);
    assert_eq!(ended(&a), &Err(SessionError::Aborted));
    assert_eq!(ended(&abort(a.state)), &Err(SessionError::Aborted));
    let t = model(new(), calls(vec![call("c1", "echo", json!(1))]));
    assert_eq!(t.state.phase, Phase::AwaitingTools);
    let a = abort(t.state);
    assert!(a.state.pending.is_empty() && a.state.phase == Phase::Done);
}

#[test]
fn stale_inputs_after_the_end_are_a_typed_error_not_a_panic() {
    let a = abort(new().state);
    let o = absorb(a.state.clone(), &spec(), &validate, Ok(say("late")));
    assert!(matches!(ended(&o), Err(SessionError::Protocol(_))));
    assert!(matches!(ended(&tools_done(a.state, &spec(), vec![])), Err(SessionError::Protocol(_))));
}

#[test]
fn max_turns_ends_the_run() {
    let cfg = Config { max_turns: 3, ..Config::default() };
    let mut o = begin(start(SessionId(1), cfg, "go"), &spec());
    let mut n = 0;
    while !matches!(o.step, Step::End(_)) {
        o = model(o, calls(vec![call("c", "echo", json!(1))]));
        o = tools(o, vec![done("c", json!(1))]);
        n += 1;
    }
    assert_eq!((n, ended(&o)), (3, &Err(SessionError::MaxTurns)));
}

#[test]
fn unknown_tool_is_answered_in_the_core_and_results_keep_source_order() {
    let o = model(new(), calls(vec![call("a", "echo", json!(1)), call("b", "nope", json!({})), call("fa", "final_answer", json!({"result": good()})), call("c", "add", json!(2))]));
    let Step::Tools(run) = &o.step else { panic!() };
    assert_eq!(run.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["a", "c"]); // unknown and final_answer never leave the core
    let o = tools_done(o.state, &spec(), vec![done("c", json!(2)), done("a", json!(1))]); // completion order differs
    assert_eq!(ended(&o), &Ok(good()));
    let ids: Vec<&str> = o.state.messages.iter().filter_map(|m| if let Message::ToolResult { call_id, .. } = m { Some(call_id.as_str()) } else { None }).collect();
    assert_eq!(ids, ["a", "b", "fa", "c"]);
    assert!(o.state.messages.iter().any(|m| matches!(m, Message::ToolResult { content, is_error: true, .. } if content == "unknown tool nope")));
}

#[test]
fn model_failure_maps_to_an_error() {
    let o = absorb(new().state, &spec(), &validate, Err("boom".into()));
    assert_eq!(ended(&o), &Err(SessionError::ModelFailed("boom".into())));
}

#[test]
fn state_survives_a_json_round_trip_at_every_step() {
    // Serialise after every transition, drop everything, deserialise, carry on: the core
    // keeps nothing between calls, so the run cannot tell.
    let hop = |o: Out| -> Out {
        let json = serde_json::to_string(&o.state).unwrap();
        Out { state: serde_json::from_str(&json).unwrap(), ..o }
    };
    let o = hop(new());
    let o = hop(model(o, calls(vec![call("c1", "add", json!({"a": 2, "b": 3}))])));
    let o = hop(tools(o, vec![done("c1", json!(5))]));
    let o = hop(model(o, answer(good())));
    assert_eq!(ended(&o), &Ok(good()));
}

#[test]
fn events_describe_the_run() {
    let o = model(new(), calls(vec![call("c1", "echo", json!(1))]));
    assert!(o.events.contains(&Event::Tokens { session: SessionId(1), tokens: 10 }));
    assert!(o.events.contains(&Event::ToolStart { session: SessionId(1), name: "echo".into() }));
    let o = tools(o, vec![done("c1", json!(1))]);
    assert!(o.events.contains(&Event::ToolEnd { session: SessionId(1), name: "echo".into(), is_error: false }));
    assert!(o.events.contains(&Event::TurnStart { session: SessionId(1), turn: 1 }));
}

#[test]
fn resume_rebuilds_the_pending_step_from_data_alone() {
    let roundtrip = |s: State| -> State { serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap() };
    // taken while a model call was in flight: resume re-asks the same request
    let o = new();
    let (before, o2) = (req(&o).clone(), resume(roundtrip(o.state), &spec()));
    assert_eq!(serde_json::to_string(&before).unwrap(), serde_json::to_string(req(&o2)).unwrap());
    // taken while a tool batch was in flight: resume re-runs the batch
    let o = model(o2, calls(vec![call("c1", "echo", json!(1))]));
    let o = resume(roundtrip(o.state), &spec());
    assert!(matches!(&o.step, Step::Tools(c) if c[0].id == "c1"));
    assert_eq!(ended(&model(tools(o, vec![done("c1", json!(1))]), answer(good()))), &Ok(good()));
    // a finished run cannot be resumed
    assert!(matches!(ended(&resume(abort(new().state).state, &spec())), Err(SessionError::Protocol(_))));
}
