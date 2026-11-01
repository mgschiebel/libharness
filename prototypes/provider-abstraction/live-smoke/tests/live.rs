//! LIVE smoke test for the Anthropic integration. ALL TESTS ARE `#[ignore]`: they only run on request,
//! and they skip (with a message) unless `ANTHROPIC_API_KEY` is set. They have never been run by the author
//! of this prototype (no key, no reliable network); this is the pending half of ticket "Prototype: provider
//! trait against the Anthropic Messages API".
//!
//!   ANTHROPIC_API_KEY=... cargo test --manifest-path live-smoke/Cargo.toml -- --ignored --nocapture
//!   (optional) ANTHROPIC_MODEL=claude-haiku-4-5-20251001
//!
//! Costs a few hundred tokens. The key is never printed.
use futures::executor::block_on;
use live_smoke::UreqTransport;
use provider_adapter::provider_codec::anthropic::Anthropic;
use provider_adapter::provider_codec::*;
use provider_adapter::complete;

fn key() -> Option<String> {
    let k = std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.trim().is_empty());
    if k.is_none() {
        eprintln!("SKIPPED: ANTHROPIC_API_KEY is not set");
    }
    k
}
fn model() -> String {
    std::env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| "claude-haiku-4-5-20251001".into())
}
fn user(t: &str) -> Message {
    Message::User(t.into())
}

#[test]
#[ignore = "live API call; needs ANTHROPIC_API_KEY"]
fn live_text_stream() {
    let Some(k) = key() else { return };
    let p = Anthropic::new(&k, &model(), 64);
    let req = ModelRequest { system: Some("Answer with a single word.".into()), messages: vec![user("Say pong.")], ..Default::default() };
    let deltas = std::sync::Mutex::new(0usize);
    let r = block_on(complete(&p, &UreqTransport::new(), &req, &|e| {
        if matches!(e, StreamEvent::Text(_)) {
            *deltas.lock().unwrap() += 1;
        }
    }))
    .unwrap_or_else(|e| panic!("live call failed: {e} / {:?}", e.metadata));
    eprintln!("text={:?} finish={:?} usage={:?} model={:?} text_deltas={}", r.text, r.finish, r.usage, r.model, deltas.lock().unwrap());
    assert!(!r.text.trim().is_empty());
    assert!(matches!(r.finish, FinishReason::Stop | FinishReason::Length));
    assert!(r.usage.as_ref().is_some_and(|u| u.prompt_tokens > 0 && u.completion_tokens > 0));
    assert!(r.model.is_some());
}

#[test]
#[ignore = "live API call; needs ANTHROPIC_API_KEY"]
fn live_tool_call_and_result_round_trip() {
    let Some(k) = key() else { return };
    let p = Anthropic::new(&k, &model(), 256);
    let t = UreqTransport::new();
    let tools = vec![ToolSpec {
        name: "get_weather".into(),
        description: "Get the current weather in a given location".into(),
        params: serde_json::json!({"type":"object","properties":{"location":{"type":"string","description":"City, e.g. San Francisco, CA"}},"required":["location"]}),
    }];
    let mut req = ModelRequest {
        messages: vec![user("What is the weather like in San Francisco?")],
        tools,
        extra: Some(serde_json::json!({"tool_choice": {"type": "any"}})),
        ..Default::default()
    };
    let r1 = block_on(complete(&p, &t, &req, &|_| {})).unwrap_or_else(|e| panic!("turn 1 failed: {e} / {:?}", e.metadata));
    eprintln!("turn1 finish={:?} calls={:?} reasoning_blocks={}", r1.finish, r1.tool_calls, r1.reasoning.len());
    assert_eq!(r1.finish, FinishReason::ToolCalls);
    let call = r1.tool_calls.first().expect("a tool call");
    assert_eq!(call.name, "get_weather");
    assert!(call.args_error.is_none() && call.args.get("location").is_some(), "args: {:?}", call.args);

    // second turn: replay the assistant turn (with any thinking blocks) and answer the tool call
    req.messages.push(Message::Assistant { text: r1.text.clone(), tool_calls: r1.tool_calls.clone(), reasoning: r1.reasoning.clone() });
    req.messages.push(Message::ToolResult { call_id: call.id.clone(), name: call.name.clone(), content: "15 degrees Celsius, foggy".into(), is_error: false });
    req.extra = None;
    let r2 = block_on(complete(&p, &t, &req, &|_| {})).unwrap_or_else(|e| panic!("turn 2 failed: {e} / {:?}", e.metadata));
    eprintln!("turn2 text={:?} finish={:?}", r2.text, r2.finish);
    assert!(!r2.text.trim().is_empty());
}

#[test]
#[ignore = "live API call; needs ANTHROPIC_API_KEY (used only to gate the run; a deliberately invalid key is what gets sent)"]
fn live_bad_key_is_an_auth_error_with_a_request_id() {
    if key().is_none() {
        return;
    }
    let p = Anthropic::new("sk-ant-invalid-key-for-smoke-test", &model(), 16);
    let req = ModelRequest { messages: vec![user("hi")], ..Default::default() };
    let e = block_on(complete(&p, &UreqTransport::new(), &req, &|_| {})).unwrap_err();
    eprintln!("kind={:?} status={:?} message={:?} metadata={:?}", e.kind, e.status, e.message, e.metadata);
    assert_eq!((e.kind, e.status), (ErrorKind::Auth, Some(401)));
    assert!(e.metadata.as_ref().is_some_and(|m| m.get("request_id").is_some()));
}
