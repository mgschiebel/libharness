//! Fixtures are SSE transcripts built from the documented formats (NOT captured from the live API).
use futures::executor::block_on;
use futures::FutureExt;
use provider_adapter::*;
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
            Message::Assistant { text: String::new(), tool_calls: vec![ToolCall { id: "c1".into(), name: "add".into(), args: serde_json::json!({"a": 1}), args_error: None }] },
            Message::ToolResult { call_id: "c1".into(), name: "add".into(), content: "3".into(), is_error: false },
        ],
        tools: vec![ToolSpec { name: "add".into(), description: "adds".into(), params: serde_json::json!({"type": "object"}) }],
        extra: Some(serde_json::json!({"reasoning": {"effort": "low"}})),
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
    assert_eq!(b["messages"][1]["content"], serde_json::Value::Null);
    assert_eq!(b["messages"][1]["tool_calls"][0]["function"]["arguments"], "{\"a\":1}");
    assert_eq!(b["messages"][2]["role"], "tool");
    assert_eq!(b["messages"][2]["tool_call_id"], "c1");
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
