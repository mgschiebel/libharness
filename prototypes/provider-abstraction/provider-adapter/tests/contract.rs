//! Fixtures are SSE transcripts built from the documented formats (NOT captured from the live API).
use futures::executor::block_on;
use futures::FutureExt;
use provider_adapter::*;
use provider_codec::anthropic::{Anthropic, Auth};
use provider_codec::openrouter::OpenRouter;
use provider_codec::stub::StubApi;
use provider_codec::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

// ---- in-memory transport -------------------------------------------------------------
struct Fake {
    status: u16,
    headers: Vec<(String, String)>,
    chunks: Vec<Vec<u8>>,
    hang_after: bool, // after the chunks, never finish (for the cancel test)
    dropped: Arc<AtomicBool>,
    sent: Arc<Mutex<Vec<HttpRequest>>>,
}
struct FakeBody(Vec<Vec<u8>>, bool, Arc<AtomicBool>);
impl Drop for FakeBody {
    fn drop(&mut self) {
        self.2.store(true, Ordering::SeqCst);
    }
}
impl Body for FakeBody {
    fn next_chunk(&mut self) -> BoxFut<Option<Result<Vec<u8>, String>>> {
        if !self.0.is_empty() {
            let c = self.0.remove(0);
            return Box::pin(async move { Some(Ok(c)) });
        }
        if self.1 {
            return Box::pin(std::future::pending());
        }
        Box::pin(async { None })
    }
}
impl Transport for Fake {
    fn send(&self, req: HttpRequest) -> BoxFut<Result<(ResponseHead, Box<dyn Body>), String>> {
        self.sent.lock().unwrap().push(req);
        let head = ResponseHead { status: self.status, headers: self.headers.clone() };
        let b: Box<dyn Body> = Box::new(FakeBody(self.chunks.clone(), self.hang_after, self.dropped.clone()));
        Box::pin(async move { Ok((head, b)) })
    }
}
fn fake(status: u16, headers: &[(&str, &str)], bytes: &str, chunk: usize) -> Fake {
    let chunks = bytes.as_bytes().chunks(chunk.max(1)).map(|c| c.to_vec()).collect();
    Fake {
        status,
        headers: headers.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
        chunks,
        hang_after: false,
        dropped: Default::default(),
        sent: Default::default(),
    }
}
fn run<P: Provider>(p: &P, f: &Fake) -> Result<ModelReply, ProviderError> {
    block_on(complete(p, f, &ModelRequest { messages: vec![Message::User("hi".into())], ..Default::default() }, &|_| {}))
}
const SIZES: [usize; 7] = [1, 2, 3, 5, 17, 64, 100_000];
fn or() -> OpenRouter {
    OpenRouter::new("sk-test", "openai/gpt-4o")
}

// ---- OpenRouter fixtures -----------------------------------------------------------------
const TEXT: &str = concat!(
    ": OPENROUTER PROCESSING\n\n",
    "data: {\"id\":\"gen-1\",\"object\":\"chat.completion.chunk\",\"model\":\"openai/gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"h\u{e9}llo \"},\"finish_reason\":null}]}\n\n",
    ": OPENROUTER PROCESSING\n\n",
    "data: {\"id\":\"gen-1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"w\u{f6}rld \u{1F600}\"},\"finish_reason\":null}]}\r\n\r\n",
    "data: {\"id\":\"gen-1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"stop\",\"native_finish_reason\":\"stop\"}]}\n\n",
    "data: {\"id\":\"gen-1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"stop\",\"native_finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":4,\"total_tokens\":9,\"cost\":0.0001}}\n\n",
    "data: [DONE]\n\n"
);
fn tc(index: usize, id: Option<&str>, name: Option<&str>, args: &str) -> String {
    let mut c = serde_json::json!({"index": index, "function": {"arguments": args}});
    if let Some(id) = id {
        c["id"] = id.into();
        c["type"] = "function".into();
    }
    if let Some(n) = name {
        c["function"]["name"] = n.into();
    }
    format!("data: {}\n\n", serde_json::json!({"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[c]},"finish_reason":null}]}))
}
fn tools_stream() -> String {
    // two parallel calls, interleaved, arguments split mid-token, name/id only in the first fragment
    let mut s = String::from(": OPENROUTER PROCESSING\n\n");
    s += &tc(0, Some("call_a"), Some("add"), "");
    s += &tc(1, Some("call_b"), Some("echo"), "{\"te");
    s += &tc(0, None, None, "{\"a\": 1, ");
    s += &tc(0, None, None, "\"b\": 2}");
    s += &tc(1, None, None, "xt\": \"h\u{e9}\"}");
    s += "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}}\n\ndata: [DONE]\n\n";
    s
}
const MIDERR: &str = concat!(
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial \"},\"finish_reason\":null}]}\n\n",
    ": OPENROUTER PROCESSING\n\n",
    "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"error\":{\"code\":429,\"message\":\"Rate limit exceeded\",\"metadata\":{\"error_type\":\"rate_limit_exceeded\"}},\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"error\"}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ignored after error\"},\"finish_reason\":null}]}\n\n"
);

#[test]
fn text_stream_with_comments_dup_finish_and_utf8_at_every_chunk_size() {
    for sz in SIZES {
        let r = run(&or(), &fake(200, &[], TEXT, sz)).unwrap_or_else(|e| panic!("size {sz}: {e}"));
        assert_eq!(r.text, "h\u{e9}llo w\u{f6}rld \u{1F600}", "size {sz}");
        assert_eq!(r.finish, FinishReason::Stop);
        assert_eq!(r.usage, Some(Usage { prompt_tokens: 5, completion_tokens: 4, total_tokens: 9, cost: Some(0.0001) }));
        assert_eq!(r.model.as_deref(), Some("openai/gpt-4o"));
        assert_eq!(r.tokens(), 9);
    }
}
#[test]
fn streaming_tool_call_deltas_across_chunks() {
    let s = tools_stream();
    for sz in SIZES {
        let r = run(&or(), &fake(200, &[], &s, sz)).unwrap();
        assert_eq!(r.finish, FinishReason::ToolCalls);
        assert_eq!(r.tool_calls.len(), 2, "size {sz}");
        assert_eq!((r.tool_calls[0].id.as_str(), r.tool_calls[0].name.as_str()), ("call_a", "add"));
        assert_eq!(r.tool_calls[0].args, serde_json::json!({"a": 1, "b": 2}));
        assert_eq!(r.tool_calls[1].args, serde_json::json!({"text": "h\u{e9}"}));
        assert!(r.tool_calls.iter().all(|c| c.args_error.is_none()));
    }
}
#[test]
fn malformed_and_empty_tool_arguments() {
    let mut s = tc(0, Some("c0"), Some("bad"), "{\"a\": ");
    s += &tc(1, Some("c1"), Some("noargs"), "");
    s += "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n";
    let r = run(&or(), &fake(200, &[], &s, 7)).unwrap();
    assert!(r.tool_calls[0].args_error.is_some());
    assert_eq!(r.tool_calls[1].args, serde_json::json!({}));
}
#[test]
fn mid_stream_error_inside_http_200_keeps_partial() {
    for sz in SIZES {
        let e = run(&or(), &fake(200, &[], MIDERR, sz)).unwrap_err();
        assert_eq!(e.kind, ErrorKind::RateLimited, "size {sz}");
        assert_eq!(e.code, Some(429));
        assert!(e.retryable());
        assert_eq!(e.partial.as_ref().unwrap().text, "partial ", "later content is ignored");
        assert_eq!(e.metadata.unwrap()["error_type"], "rate_limit_exceeded");
    }
}
#[test]
fn error_chunk_as_the_only_event() {
    let s = "data: {\"error\":{\"code\":502,\"message\":\"upstream down\"},\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"error\"}]}\n\n";
    let e = run(&or(), &fake(200, &[], s, 4)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Upstream);
    assert!(e.partial.unwrap().text.is_empty());
}
#[test]
fn finish_reason_error_without_error_object_is_still_a_failure() {
    let s = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"error\"}]}\n\ndata: [DONE]\n\n";
    assert_eq!(run(&or(), &fake(200, &[], s, 9)).unwrap_err().kind, ErrorKind::Upstream);
}
#[test]
fn truncated_stream_is_an_error_with_partial() {
    let s = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"abc\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"index\":0,\"del";
    let e = run(&or(), &fake(200, &[], s, 6)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Protocol); // the cut-off JSON event
    assert_eq!(e.partial.unwrap().text, "abc");
    let s2 = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"abc\"},\"finish_reason\":null}]}\n\n";
    assert_eq!(run(&or(), &fake(200, &[], s2, 6)).unwrap_err().kind, ErrorKind::Truncated);
}
#[test]
fn http_errors_429_retry_after_and_402_not_retryable() {
    let body = "{\"error\":{\"code\":429,\"message\":\"slow down\",\"metadata\":{\"error_type\":\"rate_limit_exceeded\"}}}";
    let e = run(&or(), &fake(429, &[("Retry-After", "7")], body, 5)).unwrap_err();
    assert_eq!((e.kind, e.retry_after_secs, e.retryable(), e.status), (ErrorKind::RateLimited, Some(7), true, Some(429)));
    let e = run(&or(), &fake(402, &[], "{\"error\":{\"code\":402,\"message\":\"no credits\"}}", 5)).unwrap_err();
    assert_eq!((e.kind, e.retryable()), (ErrorKind::Credits, false));
    let e = run(&or(), &fake(503, &[], "<html>bad gateway</html>", 5)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Upstream);
}
#[test]
fn http_200_with_plain_json_error_body() {
    let e = run(&or(), &fake(200, &[], "{\"error\":{\"code\":402,\"message\":\"out of credit\"}}", 3)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Credits);
}
#[test]
fn dropping_the_call_future_drops_the_body() {
    let mut f = fake(200, &[], TEXT.split("data: [DONE]").next().unwrap(), 50);
    f.hang_after = true;
    let dropped = f.dropped.clone();
    let p = or();
    let req = ModelRequest::default();
    let mut fut = Box::pin(complete(&p, &f, &req, &|_| {}));
    assert!(fut.as_mut().now_or_never().is_none(), "stream hangs after its chunks");
    assert!(!dropped.load(Ordering::SeqCst));
    drop(fut); // = session abort
    assert!(dropped.load(Ordering::SeqCst), "connection (body) closed on drop");
}
#[test]
fn sink_sees_live_deltas() {
    let seen = Mutex::new(Vec::new());
    let f = fake(200, &[], TEXT, 11);
    block_on(complete(&or(), &f, &ModelRequest::default(), &|e| {
        if let StreamEvent::Text(t) = e {
            seen.lock().unwrap().push(t.clone());
        }
    }))
    .unwrap();
    assert_eq!(seen.into_inner().unwrap().concat(), "h\u{e9}llo w\u{f6}rld \u{1F600}");
}

// ---- request encoding ---------------------------------------------------------------------
fn convo() -> ModelRequest {
    ModelRequest {
        messages: vec![
            Message::User("add".into()),
            Message::Assistant { text: String::new(), tool_calls: vec![ToolCall { id: "c1".into(), name: "add".into(), args: serde_json::json!({"a": 1}), args_error: None }], reasoning: vec![] },
            Message::ToolResult { call_id: "c1".into(), name: "add".into(), content: "3".into(), is_error: false },
        ],
        tools: vec![ToolSpec { name: "add".into(), description: "adds".into(), params: serde_json::json!({"type": "object"}) }],
        extra: Some(serde_json::json!({"reasoning": {"effort": "low"}})),
        system: Some("be brief".into()),
    }
}
#[test]
fn openrouter_request_encoding() {
    let mut p = or();
    p.body_defaults.insert("provider".into(), serde_json::json!({"order": ["anthropic"]}));
    let r = p.request(&convo());
    assert_eq!(r.url, "https://openrouter.ai/api/v1/chat/completions");
    assert!(r.headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer sk-test"));
    let b: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(b["stream"], true);
    assert_eq!(b["messages"][0], serde_json::json!({"role": "system", "content": "be brief"}));
    assert_eq!(b["messages"][2]["content"], serde_json::Value::Null);
    assert_eq!(b["messages"][2]["tool_calls"][0]["function"]["arguments"], "{\"a\":1}");
    assert_eq!(b["messages"][3]["role"], "tool");
    assert_eq!(b["messages"][3]["tool_call_id"], "c1");
    assert_eq!(b["tools"][0]["function"]["name"], "add");
    assert_eq!(b["provider"]["order"][0], "anthropic");
    assert_eq!(b["reasoning"]["effort"], "low");
}

// ---- the second (stub) integration: same trait, same driver ---------------------------------
fn stub() -> StubApi {
    StubApi { api_key: "k".into(), base_url: "https://stub.invalid".into() }
}
#[test]
fn stub_integration_text_tool_call_and_errors_behind_the_unchanged_trait() {
    let ok = r#"{"reply":{"text":"sure","requests":[{"ref":"r1","fn":"add","input":{"a":1,"b":2}}]},"stop":"call","used":{"in":3,"out":4}}"#;
    for sz in [1, 7, 1000] {
        let r = run(&stub(), &fake(200, &[], ok, sz)).unwrap();
        assert_eq!(r.text, "sure");
        assert_eq!(r.tool_calls[0].id, "r1");
        assert_eq!(r.tool_calls[0].args, serde_json::json!({"a": 1, "b": 2}));
        assert_eq!((&r.finish, r.tokens()), (&FinishReason::ToolCalls, 7));
    }
    let e = run(&stub(), &fake(429, &[], r#"{"problem":"too fast"}"#, 4)).unwrap_err();
    assert_eq!((e.kind, e.message.as_str()), (ErrorKind::RateLimited, "too fast"));
    assert_eq!(run(&stub(), &fake(200, &[], "{\"reply\":", 4)).unwrap_err().kind, ErrorKind::Protocol);
    let b: serde_json::Value = serde_json::from_slice(&stub().request(&convo()).body).unwrap();
    assert_eq!(b["turns"][2]["who"], "tool");
    assert_eq!(b["functions"][0]["about"], "adds");
}

// ---- the callback shape the session loop registers ------------------------------------------
#[test]
fn model_callback_has_session_task_shape_and_works_for_both_providers() {
    fn call<P: Provider + Send + Sync + 'static>(p: P, f: Fake) -> Result<ModelReply, String>
    where
        P::Decoder: Send,
    {
        let cb = model_callback(Arc::new(p), Arc::new(f));
        block_on(cb(ModelRequest::default()))
    }
    assert_eq!(call(or(), fake(200, &[], TEXT, 13)).unwrap().tokens(), 9);
    assert!(call(or(), fake(200, &[], MIDERR, 13)).unwrap_err().contains("RateLimited"));
    assert_eq!(call(stub(), fake(200, &[], r#"{"reply":{"text":"x"},"stop":"end","used":{"in":1,"out":1}}"#, 5)).unwrap().text, "x");
}

// =====================================================================================================
// Anthropic Messages API. Fixtures are built from the event formats printed in the official streaming
// page (see README); NOTHING here ran against the live API.
// =====================================================================================================
fn an() -> Anthropic {
    Anthropic::new("sk-ant-test", "claude-test", 1024)
}
/// One SSE event the way the docs print them: `event: <name>` line, `data:` line, blank line.
fn ev(name: &str, data: serde_json::Value) -> String {
    format!("event: {name}\ndata: {data}\n\n")
}
fn start(input: u64) -> String {
    ev("message_start", serde_json::json!({"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"claude-test","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":input,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":1}}}))
}
fn block_start(i: usize, b: serde_json::Value) -> String {
    ev("content_block_start", serde_json::json!({"type":"content_block_start","index":i,"content_block":b}))
}
fn delta(i: usize, d: serde_json::Value) -> String {
    ev("content_block_delta", serde_json::json!({"type":"content_block_delta","index":i,"delta":d}))
}
fn stop(i: usize) -> String {
    ev("content_block_stop", serde_json::json!({"type":"content_block_stop","index":i}))
}
fn msg_delta(reason: &str, out: u64) -> String {
    ev("message_delta", serde_json::json!({"type":"message_delta","delta":{"stop_reason":reason,"stop_sequence":null},"usage":{"output_tokens":out}}))
}
fn ping() -> String {
    ev("ping", serde_json::json!({"type":"ping"}))
}
fn msg_stop() -> String {
    ev("message_stop", serde_json::json!({"type":"message_stop"}))
}
fn text_d(i: usize, t: &str) -> String {
    delta(i, serde_json::json!({"type":"text_delta","text":t}))
}
fn json_d(i: usize, t: &str) -> String {
    delta(i, serde_json::json!({"type":"input_json_delta","partial_json":t}))
}

fn an_text_stream() -> String {
    start(25) + &block_start(0, serde_json::json!({"type":"text","text":""})) + &ping() + &text_d(0, "h\u{e9}llo ") + &ping()
        + &text_d(0, "w\u{f6}rld \u{1F600}") + &stop(0) + &msg_delta("end_turn", 15) + &msg_stop()
}
/// text block, then a tool_use at index 1, then a second parallel tool at index 2, args split mid-token.
fn an_tool_stream() -> String {
    start(472)
        + &block_start(0, serde_json::json!({"type":"text","text":""}))
        + &text_d(0, "Okay, checking")
        + &stop(0)
        + &block_start(1, serde_json::json!({"type":"tool_use","id":"toolu_01","name":"get_weather","input":{}}))
        + &json_d(1, "")
        + &json_d(1, "{\"location\":")
        + &json_d(1, " \"San Fr\u{e9}")
        + &ping()
        + &json_d(1, "o, CA\"}")
        + &stop(1)
        + &block_start(2, serde_json::json!({"type":"tool_use","id":"toolu_02","name":"noargs","input":{}}))
        + &json_d(2, "")
        + &stop(2)
        + &msg_delta("tool_use", 89)
        + &msg_stop()
}

#[test]
fn anthropic_text_stream_named_events_ping_utf8_and_cumulative_usage() {
    for sz in SIZES {
        let r = run(&an(), &fake(200, &[], &an_text_stream(), sz)).unwrap_or_else(|e| panic!("size {sz}: {e}"));
        assert_eq!(r.text, "h\u{e9}llo w\u{f6}rld \u{1F600}", "size {sz}");
        assert_eq!(r.finish, FinishReason::Stop);
        // input 25 came from message_start; message_delta carried only output_tokens (cumulative, replaces 1)
        assert_eq!(r.usage, Some(Usage { prompt_tokens: 25, completion_tokens: 15, total_tokens: 40, cost: None }));
        assert_eq!(r.model.as_deref(), Some("claude-test"));
    }
}
#[test]
fn anthropic_tool_use_blocks_with_partial_json_deltas() {
    let s = an_tool_stream();
    for sz in SIZES {
        let r = run(&an(), &fake(200, &[], &s, sz)).unwrap();
        assert_eq!(r.text, "Okay, checking");
        assert_eq!(r.finish, FinishReason::ToolCalls);
        assert_eq!(r.tool_calls.len(), 2, "size {sz}");
        assert_eq!((r.tool_calls[0].id.as_str(), r.tool_calls[0].name.as_str()), ("toolu_01", "get_weather"));
        assert_eq!(r.tool_calls[0].args, serde_json::json!({"location": "San Fr\u{e9}o, CA"}));
        assert_eq!((r.tool_calls[1].name.as_str(), &r.tool_calls[1].args), ("noargs", &serde_json::json!({})));
        assert!(r.tool_calls.iter().all(|c| c.args_error.is_none()));
    }
}
#[test]
fn anthropic_thinking_block_with_signature_and_redacted_block() {
    let s = start(10)
        + &block_start(0, serde_json::json!({"type":"thinking","thinking":""}))
        + &delta(0, serde_json::json!({"type":"thinking_delta","thinking":"I need the GCD.\n"}))
        + &delta(0, serde_json::json!({"type":"thinking_delta","thinking":"1071 = 2 \u{d7} 462 + 147"}))
        + &delta(0, serde_json::json!({"type":"signature_delta","signature":"EqQBCgIY"}))
        + &stop(0)
        + &block_start(1, serde_json::json!({"type":"redacted_thinking","data":"ENCRYPTED"}))
        + &stop(1)
        + &block_start(2, serde_json::json!({"type":"text","text":""}))
        + &text_d(2, "21")
        + &stop(2)
        + &msg_delta("end_turn", 20)
        + &msg_stop();
    for sz in [1, 7, 100_000] {
        let r = run(&an(), &fake(200, &[], &s, sz)).unwrap();
        assert_eq!(r.text, "21");
        assert_eq!(
            r.reasoning,
            vec![
                ReasoningBlock::Thinking { text: "I need the GCD.\n1071 = 2 \u{d7} 462 + 147".into(), signature: "EqQBCgIY".into() },
                ReasoningBlock::Redacted { data: "ENCRYPTED".into() },
            ]
        );
    }
    // display "omitted": empty thinking_delta then a single signature_delta
    let s = start(1) + &block_start(0, serde_json::json!({"type":"thinking","thinking":""}))
        + &delta(0, serde_json::json!({"type":"thinking_delta","thinking":""}))
        + &delta(0, serde_json::json!({"type":"signature_delta","signature":"SIG"})) + &stop(0) + &msg_delta("end_turn", 2) + &msg_stop();
    let r = run(&an(), &fake(200, &[], &s, 5)).unwrap();
    assert_eq!(r.reasoning, vec![ReasoningBlock::Thinking { text: String::new(), signature: "SIG".into() }]);
}
#[test]
fn anthropic_unknown_block_delta_and_event_types_are_skipped() {
    let s = start(1)
        + &block_start(0, serde_json::json!({"type":"server_tool_use","id":"srvtoolu_1","name":"web_search","input":{}}))
        + &json_d(0, "{\"query\":\"x\"}")
        + &stop(0)
        + &ev("some_future_event", serde_json::json!({"type":"some_future_event"}))
        + &block_start(1, serde_json::json!({"type":"text","text":""}))
        + &delta(1, serde_json::json!({"type":"citations_delta","citation":{}}))
        + &text_d(1, "ok")
        + &stop(1)
        + &msg_delta("end_turn", 3)
        + &msg_stop();
    let r = run(&an(), &fake(200, &[], &s, 13)).unwrap();
    assert_eq!((r.text.as_str(), r.tool_calls.len()), ("ok", 0), "server-tool args must not become a client tool call");
}
#[test]
fn anthropic_stop_reasons_map_to_the_shared_finish_reason() {
    for (wire, want) in [
        ("end_turn", FinishReason::Stop),
        ("stop_sequence", FinishReason::Stop),
        ("tool_use", FinishReason::ToolCalls),
        ("max_tokens", FinishReason::Length),
        ("model_context_window_exceeded", FinishReason::Length),
        ("refusal", FinishReason::ContentFilter),
        ("pause_turn", FinishReason::Other("pause_turn".into())),
    ] {
        let s = start(1) + &msg_delta(wire, 1) + &msg_stop();
        assert_eq!(run(&an(), &fake(200, &[], &s, 9)).unwrap().finish, want, "{wire}");
    }
}
#[test]
fn anthropic_error_event_mid_stream_overloaded_keeps_partial_and_ignores_the_rest() {
    let s = start(5) + &block_start(0, serde_json::json!({"type":"text","text":""})) + &text_d(0, "partial ")
        + &ev("error", serde_json::json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}))
        + &text_d(0, "ignored after error");
    for sz in SIZES {
        let e = run(&an(), &fake(200, &[], &s, sz)).unwrap_err();
        assert_eq!((e.kind, e.message.as_str(), e.status, e.retryable()), (ErrorKind::Upstream, "Overloaded", None, true), "size {sz}");
        assert_eq!(e.metadata.as_ref().unwrap()["error_type"], "overloaded_error");
        let p = e.partial.unwrap();
        assert_eq!(p.text, "partial ");
        assert_eq!(p.usage.unwrap().prompt_tokens, 5, "usage seen before the error is kept");
    }
}
#[test]
fn anthropic_truncated_stream_with_or_without_message_delta() {
    // dies mid-text
    let s = start(1) + &block_start(0, serde_json::json!({"type":"text","text":""})) + &text_d(0, "abc");
    let e = run(&an(), &fake(200, &[], &s, 6)).unwrap_err();
    assert_eq!((e.kind, e.partial.unwrap().text.as_str()), (ErrorKind::Truncated, "abc"));
    // message_delta arrived but message_stop did not: still not a clean end
    let s = start(1) + &msg_delta("end_turn", 1);
    assert_eq!(run(&an(), &fake(200, &[], &s, 6)).unwrap_err().kind, ErrorKind::Truncated);
    // cut inside a JSON event
    let s = start(1) + "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"ind";
    assert_eq!(run(&an(), &fake(200, &[], &s, 6)).unwrap_err().kind, ErrorKind::Protocol);
}
#[test]
fn anthropic_non_2xx_error_bodies() {
    let body = |ty: &str, extra: &str| format!(r#"{{"type":"error","error":{{"type":"{ty}","message":"m"{extra}}},"request_id":"req_011"}}"#);
    let kinds = [
        (400, "invalid_request_error", ErrorKind::BadRequest, false),
        (401, "authentication_error", ErrorKind::Auth, false),
        (402, "billing_error", ErrorKind::Credits, false),
        (403, "permission_error", ErrorKind::Auth, false),
        (404, "not_found_error", ErrorKind::BadRequest, false),
        (413, "request_too_large", ErrorKind::BadRequest, false),
        (429, "rate_limit_error", ErrorKind::RateLimited, true),
        (500, "api_error", ErrorKind::Upstream, true),
        (504, "timeout_error", ErrorKind::Upstream, true),
        (529, "overloaded_error", ErrorKind::Upstream, true),
    ];
    for (status, ty, kind, retry) in kinds {
        let e = run(&an(), &fake(status, &[("request-id", "req_hdr")], &body(ty, ""), 5)).unwrap_err();
        assert_eq!((e.kind, e.retryable(), e.status, e.code), (kind, retry, Some(status), Some(status as i64)), "{ty}");
        assert_eq!(e.metadata.as_ref().unwrap()["request_id"], "req_011", "body request_id wins over header");
    }
    let e = run(&an(), &fake(429, &[("retry-after", "30")], &body("rate_limit_error", ""), 5)).unwrap_err();
    assert_eq!((e.kind, e.retry_after_secs), (ErrorKind::RateLimited, Some(30)));
    // spend-cap 429: same error type, no retry-after, details.error_code tells it apart => not retryable
    let e = run(&an(), &fake(429, &[], &body("rate_limit_error", r#","details":{"error_code":"enforced_spend_limit_reached"}"#), 5)).unwrap_err();
    assert_eq!((e.kind, e.retryable()), (ErrorKind::Credits, false));
    // unknown future error type falls back to the status; non-JSON body (gateway page) too
    let e = run(&an(), &fake(422, &[], &body("brand_new_error", ""), 5)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::BadRequest);
    let e = run(&an(), &fake(502, &[("request-id", "req_h")], "<html>bad gateway</html>", 5)).unwrap_err();
    assert_eq!((e.kind, e.status), (ErrorKind::Upstream, Some(502)));
}
#[test]
fn anthropic_http_200_with_plain_json_error_body() {
    let e = run(&an(), &fake(200, &[], r#"{"type":"error","error":{"type":"billing_error","message":"x"}}"#, 3)).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Credits);
}

fn body_of(r: &HttpRequest) -> serde_json::Value {
    serde_json::from_slice(&r.body).unwrap()
}
#[test]
fn anthropic_request_encoding_headers_system_max_tokens_tools() {
    let mut p = an();
    let r = p.request(&convo());
    assert_eq!(r.url, "https://api.anthropic.com/v1/messages");
    let h = |k: &str| r.headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    assert_eq!((h("x-api-key"), h("anthropic-version"), h("content-type"), h("Authorization")), (Some("sk-ant-test"), Some("2023-06-01"), Some("application/json"), None));
    let b = body_of(&r);
    assert_eq!((b["model"].as_str(), b["max_tokens"].as_u64(), &b["stream"], b["system"].as_str()), (Some("claude-test"), Some(1024), &serde_json::json!(true), Some("be brief")));
    assert!(b["messages"].as_array().unwrap().iter().all(|m| m["role"] != "system"), "system is top-level, never a message");
    assert_eq!(b["tools"][0], serde_json::json!({"name": "add", "description": "adds", "input_schema": {"type": "object"}}));
    assert_eq!(b["reasoning"]["effort"], "low", "extra passes through (unchecked; Anthropic would reject unknown fields, not ignore them)");
    // assistant turn: tool_use block; tool result: user message with a tool_result block
    assert_eq!(b["messages"][1], serde_json::json!({"role":"assistant","content":[{"type":"tool_use","id":"c1","name":"add","input":{"a":1}}]}));
    assert_eq!(b["messages"][2], serde_json::json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"c1","content":"3"}]}));
    // per-request override of max_tokens through extra, bearer auth variant
    p.auth = Auth::Bearer;
    p.version = "2026-10-21".into();
    let mut req = convo();
    req.extra = Some(serde_json::json!({"max_tokens": 77, "thinking": {"type": "adaptive"}}));
    let r = p.request(&req);
    assert!(r.headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer sk-ant-test"));
    assert!(r.headers.iter().all(|(k, _)| k != "x-api-key"));
    assert!(r.headers.iter().any(|(k, v)| k == "anthropic-version" && v == "2026-10-21"));
    let b = body_of(&r);
    assert_eq!((b["max_tokens"].as_u64(), &b["thinking"]["type"]), (Some(77), &serde_json::json!("adaptive")));
}
#[test]
fn anthropic_request_groups_parallel_tool_results_replays_thinking_and_marks_errors() {
    let call = |id: &str| ToolCall { id: id.into(), name: "t".into(), args: serde_json::json!({}), args_error: None };
    let bad = ToolCall { id: "toolu_3".into(), name: "t".into(), args: serde_json::Value::Null, args_error: Some("bad".into()) };
    let res = |id: &str, ok: bool| Message::ToolResult { call_id: id.into(), name: "t".into(), content: "r".into(), is_error: !ok };
    let req = ModelRequest {
        messages: vec![
            Message::User("go".into()),
            Message::Assistant {
                text: "calling".into(),
                tool_calls: vec![call("toolu_1"), call("toolu_2"), bad],
                reasoning: vec![ReasoningBlock::Thinking { text: "hmm".into(), signature: "SIG".into() }, ReasoningBlock::Redacted { data: "D".into() }],
            },
            res("toolu_1", true),
            res("toolu_2", false),
            res("toolu_3", true),
            Message::User("and now?".into()),
            Message::Assistant { text: String::new(), tool_calls: vec![], reasoning: vec![] },
        ],
        ..Default::default()
    };
    let b = body_of(&an().request(&req));
    let m = b["messages"].as_array().unwrap();
    assert_eq!(m.len(), 4, "3 results + the follow-up user text share ONE user message");
    assert_eq!(m[0], serde_json::json!({"role":"user","content":"go"}));
    let a = &m[1]["content"];
    assert_eq!(a[0], serde_json::json!({"type":"thinking","thinking":"hmm","signature":"SIG"}));
    assert_eq!(a[1], serde_json::json!({"type":"redacted_thinking","data":"D"}));
    assert_eq!(a[2], serde_json::json!({"type":"text","text":"calling"}));
    assert_eq!(a[3]["type"], "tool_use");
    assert_eq!(a[5]["input"], serde_json::json!({}), "a call with unparseable args is replayed with an empty object");
    let u = m[2]["content"].as_array().unwrap();
    assert_eq!(u.len(), 4);
    assert!(u[..3].iter().all(|b| b["type"] == "tool_result"), "tool_result blocks first");
    assert_eq!(u[1]["is_error"], true);
    assert!(u[0].get("is_error").is_none());
    assert_eq!(u[3], serde_json::json!({"type":"text","text":"and now?"}));
    assert_eq!(m[3], serde_json::json!({"role":"assistant","content":[]}), "an empty assistant turn encodes to empty content; whether the API accepts that is unverified (assumed rejected)");
}
#[test]
fn anthropic_behind_the_same_callback_and_driver() {
    let cb_ok = model_callback(Arc::new(an()), Arc::new(fake(200, &[], &an_text_stream(), 13)));
    assert_eq!(block_on(cb_ok(ModelRequest::default())).unwrap().tokens(), 40);
    let s = start(1) + &ev("error", serde_json::json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}));
    let cb_err = model_callback(Arc::new(an()), Arc::new(fake(200, &[], &s, 13)));
    assert!(block_on(cb_err(ModelRequest::default())).unwrap_err().contains("Overloaded"));
}
#[test]
fn anthropic_dropping_the_call_future_drops_the_body() {
    let mut f = fake(200, &[], &(start(1) + &block_start(0, serde_json::json!({"type":"text","text":""})) + &text_d(0, "x")), 50);
    f.hang_after = true;
    let dropped = f.dropped.clone();
    let p = an();
    let req = ModelRequest::default();
    let mut fut = Box::pin(complete(&p, &f, &req, &|_| {}));
    assert!(fut.as_mut().now_or_never().is_none());
    drop(fut);
    assert!(dropped.load(Ordering::SeqCst));
}
