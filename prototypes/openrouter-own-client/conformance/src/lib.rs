//! Conformance cases 1 to 8 of the shared protocol (plus extra checks) against any `Transport`,
//! over real loopback HTTP/1.1 served by `mock-server`. Cases are runtime-neutral futures: only
//! `exec.rs` knows which executor drives them. NOT exercised against the live API; TLS is untested.
pub mod exec;
pub mod fixtures;
pub mod threads;

use fixtures::*;
use mock_server::{MockServer, Scenario, Step};
use provider_adapter::bridge::thread_sleep;
use provider_adapter::provider_codec::openrouter::OpenRouter;
use provider_adapter::provider_codec::*;
use provider_adapter::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type T = Arc<dyn Transport>;
pub type CaseFut = BoxFut<'static, Result<String, String>>;

pub fn or(server: &MockServer) -> OpenRouter {
    let mut p = OpenRouter::new("sk-test", "openai/gpt-4o");
    p.base_url = server.base_url();
    p
}
fn req(text: &str) -> ModelRequest {
    ModelRequest { messages: vec![Message::User(text.into())], ..Default::default() }
}
fn no_sink() -> impl Fn(&StreamEvent) + Send + Sync {
    |_| {}
}
macro_rules! ensure {
    ($c:expr, $($m:tt)+) => { if !($c) { return Err(format!($($m)+)); } };
}
async fn eventually(limit: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if f() {
            return true;
        }
        thread_sleep(Duration::from_millis(10)).await;
    }
    f()
}

pub const CASES: [(&str, &str); 13] = [
    ("1", "SSE comment lines, also between events (chunk sizes 1, 7, 4096)"),
    ("2", "error chunk mid-stream in a 200, partial text kept"),
    ("3", "2 parallel tool calls, split mid-token, interleaved"),
    ("4", "usage with cost, cost_details, is_byok, duplicated finish_reason"),
    ("5", "served model differs from requested"),
    ("6", "dropping the call future closes the socket"),
    ("7", "models / provider / reasoning / plugins reach the wire"),
    ("8", "HTTP 429 + Retry-After, 402, 503 as plain error responses"),
    ("X1", "reasoning + reasoning_details merged, echoed on the next request"),
    ("X2", "incremental delivery (first delta seen before the stream ends)"),
    ("X3", "connection cut mid-body, and connection refused: how the error surfaces"),
    ("X4", "retry on 429 with a host-supplied sleeper"),
    ("X5", "callback flattening: what Err(String) keeps"),
];

pub fn run_case(id: &str, t: T) -> CaseFut {
    match id {
        "1" => Box::pin(case1(t)),
        "2" => Box::pin(case2(t)),
        "3" => Box::pin(case3(t)),
        "4" => Box::pin(case4(t)),
        "5" => Box::pin(case5(t)),
        "6" => Box::pin(case6(t)),
        "7" => Box::pin(case7(t)),
        "8" => Box::pin(case8(t)),
        "X1" => Box::pin(x1(t)),
        "X2" => Box::pin(x2(t)),
        "X3" => Box::pin(x3(t)),
        "X4" => Box::pin(x4(t)),
        "X5" => Box::pin(x5(t)),
        _ => panic!("no case {id}"),
    }
}

async fn case1(t: T) -> Result<String, String> {
    for piece in [1, 7, 4096] {
        let srv = MockServer::start(vec![Scenario::sse(TEXT.as_bytes(), piece)]);
        let r = complete(&or(&srv), &*t, &req("hi"), &no_sink()).await.map_err(|e| format!("piece {piece}: {e}"))?;
        ensure!(r.text == TEXT_DECODED, "piece {piece}: text {:?}", r.text);
        ensure!(r.finish == FinishReason::Stop, "finish");
        let rec = &srv.requests()[0];
        ensure!(rec.path == "/api/v1/chat/completions" && rec.method == "POST", "request line {} {}", rec.method, rec.path);
        ensure!(rec.header("authorization") == Some("Bearer sk-test"), "auth header lost");
        let b: Value = serde_json::from_slice(&rec.body).map_err(|e| e.to_string())?;
        ensure!(b["stream"] == true && b["messages"][0]["content"] == "hi", "body {b}");
    }
    Ok("text, finish and request line/headers/body ok at 3 chunk sizes".into())
}
async fn case2(t: T) -> Result<String, String> {
    for piece in [1, 9, 4096] {
        let mut steps: Vec<Step> = MIDERR.as_bytes().chunks(piece).map(|c| Step::Data(c.to_vec())).collect();
        steps.push(Step::Hold); // the server would keep going: the client must hang up after the error
        let srv = MockServer::start(vec![Scenario::sse_steps(steps)]);
        let e = complete(&or(&srv), &*t, &req("hi"), &no_sink()).await.err().ok_or("expected an error")?;
        ensure!(e.kind == ErrorKind::RateLimited && e.code == Some(429), "kind {:?} code {:?}", e.kind, e.code);
        ensure!(e.partial.as_ref().map(|p| p.text.as_str()) == Some("partial "), "partial {:?}", e.partial);
        ensure!(e.metadata.as_ref().map(|m| m["error_type"] == "rate_limit_exceeded") == Some(true), "metadata lost");
        ensure!(eventually(Duration::from_secs(3), || srv.client_closed(0)).await, "piece {piece}: client kept the socket open after the error chunk");
    }
    Ok("partial text recovered via ProviderError.partial; socket closed after the error".into())
}
async fn case3(t: T) -> Result<String, String> {
    let s = tools_stream();
    for piece in [1, 7, 4096] {
        let srv = MockServer::start(vec![Scenario::sse(s.as_bytes(), piece)]);
        let r = complete(&or(&srv), &*t, &req("hi"), &no_sink()).await.map_err(|e| format!("piece {piece}: {e}"))?;
        ensure!(r.finish == FinishReason::ToolCalls && r.tool_calls.len() == 2, "calls {:?}", r.tool_calls);
        ensure!(r.tool_calls[0].id == "call_a" && r.tool_calls[0].name == "add" && r.tool_calls[0].args == json!({"a": 1, "b": 2}), "call0 {:?}", r.tool_calls[0]);
        ensure!(r.tool_calls[1].args == json!({"text": "h\u{e9}"}), "call1 {:?}", r.tool_calls[1]);
    }
    Ok("2 calls assembled by index at 3 chunk sizes".into())
}
async fn case4(t: T) -> Result<String, String> {
    let srv = MockServer::start(vec![Scenario::sse(USAGE.as_bytes(), 13)]);
    let r = complete(&or(&srv), &*t, &req("hi"), &no_sink()).await.map_err(|e| e.to_string())?;
    let u = r.usage.ok_or("no usage")?;
    ensure!(r.finish == FinishReason::Stop && r.text == "ok", "finish/text {:?} {:?}", r.finish, r.text);
    ensure!(u.cost == Some(0.00123) && u.is_byok == Some(true), "cost/byok {:?}", u);
    ensure!(u.cost_details.as_ref().map(|d| d["upstream_inference_cost"] == 0.001) == Some(true), "cost_details {:?}", u.cost_details);
    ensure!(u.reasoning_tokens == Some(5) && u.cached_tokens == Some(3) && u.total_tokens == 30, "tokens {:?}", u);
    Ok("cost, cost_details (JSON), is_byok, token details kept; duplicate finish ignored".into())
}
async fn case5(t: T) -> Result<String, String> {
    let srv = MockServer::start(vec![Scenario::sse(USAGE.as_bytes(), 50)]);
    let mut q = req("hi");
    q.extra = Some(json!({"models": ["openai/gpt-4o", "anthropic/claude-sonnet-4.5"]}));
    let r = complete(&or(&srv), &*t, &q, &no_sink()).await.map_err(|e| e.to_string())?;
    ensure!(r.model.as_deref() == Some("anthropic/claude-sonnet-4.5"), "served model {:?}", r.model);
    let b: Value = serde_json::from_slice(&srv.requests()[0].body).unwrap();
    ensure!(b["model"] == "openai/gpt-4o", "requested model not on the wire");
    Ok("reply.model = served model (anthropic/...), request carried openai/gpt-4o".into())
}
async fn case6(t: T) -> Result<String, String> {
    use futures::future::{select, Either};
    let first = TEXT.split("data: [DONE]").next().unwrap().as_bytes().to_vec();
    let srv = MockServer::start(vec![Scenario::sse_steps(vec![Step::Data(first), Step::Hold])]);
    let (tx, rx) = futures::channel::oneshot::channel::<()>();
    let tx = Mutex::new(Some(tx));
    let sink = move |e: &StreamEvent| {
        if matches!(e, StreamEvent::Text(_)) {
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(());
            }
        }
    };
    let p = or(&srv);
    let q = req("hi");
    let call = Box::pin(complete(&p, &*t, &q, &sink));
    match select(call, rx).await {
        Either::Left((r, _)) => return Err(format!("call ended before cancel: {r:?}")),
        Either::Right((_, call)) => {
            ensure!(!srv.client_closed(0), "closed before drop?");
            drop(call); // = session abort
        }
    }
    ensure!(eventually(Duration::from_secs(3), || srv.client_closed(0)).await, "server-side socket still open 3 s after the call future was dropped");
    Ok("socket closed after drop".into())
}
async fn case7(t: T) -> Result<String, String> {
    let srv = MockServer::start(vec![Scenario::sse(TEXT.as_bytes(), 4096)]);
    let mut p = or(&srv);
    p.body_defaults.insert("provider".into(), json!({"order": ["anthropic"], "allow_fallbacks": false}));
    let mut q = req("hi");
    q.extra = Some(json!({
        "models": ["openai/gpt-4o", "anthropic/claude-sonnet-4.5"],
        "reasoning": {"effort": "high", "exclude": false},
        "plugins": [{"id": "web", "max_results": 3}],
        "provder": {"typo": true}
    }));
    complete(&p, &*t, &q, &no_sink()).await.map_err(|e| e.to_string())?;
    let b: Value = serde_json::from_slice(&srv.requests()[0].body).unwrap();
    ensure!(b["models"][1] == "anthropic/claude-sonnet-4.5", "models {b}");
    ensure!(b["provider"]["order"][0] == "anthropic" && b["provider"]["allow_fallbacks"] == false, "provider {b}");
    ensure!(b["reasoning"]["effort"] == "high" && b["plugins"][0]["id"] == "web", "reasoning/plugins {b}");
    ensure!(b["provder"]["typo"] == true, "typo key should pass through unchecked");
    // `extra` is inserted after the typed fields, so it can silently override them:
    let srv2 = MockServer::start(vec![Scenario::sse(TEXT.as_bytes(), 4096)]);
    let mut q2 = req("hi");
    q2.extra = Some(json!({"stream": false, "model": "evil/model"}));
    complete(&or(&srv2), &*t, &q2, &no_sink()).await.ok();
    let b2: Value = serde_json::from_slice(&srv2.requests()[0].body).unwrap();
    ensure!(b2["stream"] == false && b2["model"] == "evil/model", "expected the reserved-key override to reach the wire: {b2}");
    Ok("all four reach the wire as unchecked JSON (typo key too); extra silently overrides stream/model".into())
}
async fn case8(t: T) -> Result<String, String> {
    let srv = MockServer::start(vec![
        Scenario::fixed(429, &[("Retry-After", "7")], E429),
        Scenario::fixed(402, &[], E402),
        Scenario::fixed(503, &[], "<html>bad gateway</html>"),
    ]);
    let p = or(&srv);
    let e = complete(&p, &*t, &req("hi"), &no_sink()).await.err().ok_or("429: expected an error")?;
    ensure!((e.kind, e.retry_after_secs, e.retryable(), e.status) == (ErrorKind::RateLimited, Some(7), true, Some(429)), "429 -> {e:?}");
    ensure!(e.message == "slow down" && e.metadata.is_some(), "429 body lost: {e:?}");
    let e = complete(&p, &*t, &req("hi"), &no_sink()).await.err().ok_or("402: expected an error")?;
    ensure!((e.kind, e.retryable(), e.status) == (ErrorKind::Credits, false, Some(402)), "402 -> {e:?}");
    let e = complete(&p, &*t, &req("hi"), &no_sink()).await.err().ok_or("503: expected an error")?;
    ensure!(e.kind == ErrorKind::Upstream && e.status == Some(503), "503 -> {e:?}");
    Ok("429 (Retry-After 7, retryable), 402 (not retryable), 503 html body: all typed".into())
}
async fn x1(t: T) -> Result<String, String> {
    let srv = MockServer::start(vec![Scenario::sse(REASONING.as_bytes(), 11), Scenario::sse(TEXT.as_bytes(), 4096)]);
    let p = or(&srv);
    let r = complete(&p, &*t, &req("think"), &no_sink()).await.map_err(|e| e.to_string())?;
    ensure!(r.reasoning == "Let me think" && r.text == "done", "reasoning {:?} text {:?}", r.reasoning, r.text);
    ensure!(r.reasoning_details.len() == 2, "details {:?}", r.reasoning_details);
    ensure!(r.reasoning_details[0]["text"] == "Let me think" && r.reasoning_details[0]["signature"] == "sig123" && r.reasoning_details[0]["format"] == "anthropic-claude-v1", "merge {:?}", r.reasoning_details[0]);
    ensure!(r.reasoning_details[1]["type"] == "reasoning.encrypted", "encrypted block {:?}", r.reasoning_details[1]);
    let q = ModelRequest {
        messages: vec![Message::User("think".into()), Message::Assistant { text: r.text.clone(), tool_calls: vec![], reasoning_details: r.reasoning_details.clone() }, Message::User("go on".into())],
        ..Default::default()
    };
    complete(&p, &*t, &q, &no_sink()).await.map_err(|e| e.to_string())?;
    let b: Value = serde_json::from_slice(&srv.requests()[1].body).unwrap();
    ensure!(b["messages"][1]["reasoning_details"][0]["signature"] == "sig123", "reasoning_details not echoed: {b}");
    Ok("reasoning text, merged reasoning_details (by index), echoed on the next turn".into())
}
async fn x2(t: T) -> Result<String, String> {
    let (a, b) = TEXT.split_at(TEXT.find("data: {\"id\":\"gen-1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"w").unwrap());
    let srv = MockServer::start(vec![Scenario::sse_steps(vec![Step::Data(a.as_bytes().to_vec()), Step::Pause(Duration::from_millis(500)), Step::Data(b.as_bytes().to_vec())])]);
    let t0 = Instant::now();
    let first = Mutex::new(None::<Duration>);
    let sink = |e: &StreamEvent| {
        if let StreamEvent::Text(_) = e {
            first.lock().unwrap().get_or_insert(t0.elapsed());
        }
    };
    complete(&or(&srv), &*t, &req("hi"), &sink).await.map_err(|e| e.to_string())?;
    let (f, total) = (first.lock().unwrap().ok_or("no text event")?, t0.elapsed());
    ensure!(total >= Duration::from_millis(450) && f + Duration::from_millis(350) < total, "first delta at {f:?}, stream done at {total:?}: delivery is buffered");
    Ok(format!("first delta at {} ms, stream finished at {} ms", f.as_millis(), total.as_millis()))
}
async fn x3(t: T) -> Result<String, String> {
    let first = b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"abc\"},\"finish_reason\":null}]}\n\n".to_vec();
    let srv = MockServer::start(vec![Scenario::sse_steps(vec![Step::Data(first), Step::Pause(Duration::from_millis(50)), Step::Abort])]);
    let e = complete(&or(&srv), &*t, &req("hi"), &no_sink()).await.err().ok_or("expected an error")?;
    ensure!(e.kind == ErrorKind::Transport, "cut body -> {:?}", e.kind);
    ensure!(e.partial.as_ref().map(|p| p.text.as_str()) == Some("abc"), "partial lost: {:?}", e.partial);
    let cut = e.message.clone();
    // connection refused: bind then drop a listener to get a closed port
    let port = { std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port() };
    let mut p = or(&srv);
    p.base_url = format!("http://127.0.0.1:{port}/api/v1");
    let e2 = complete(&p, &*t, &req("hi"), &no_sink()).await.err().ok_or("expected an error")?;
    ensure!(e2.kind == ErrorKind::Transport && e2.partial.is_none(), "refused -> {e2:?}");
    Ok(format!("cut body: Transport + partial \"abc\"; message {cut:?} | refused: {:?}", e2.message))
}
async fn x4(t: T) -> Result<String, String> {
    let srv = MockServer::start(vec![Scenario::fixed(429, &[("Retry-After", "1")], E429), Scenario::sse(TEXT.as_bytes(), 4096)]);
    let p = Arc::new(or(&srv));
    let sleep: Arc<Sleep> = Arc::new(thread_sleep);
    let cb = model_callback_retrying(p, t, sleep, 3, Duration::from_secs(5));
    let t0 = Instant::now();
    let r = cb(req("hi")).await?;
    ensure!(r.text == TEXT_DECODED, "text");
    ensure!(srv.connections() == 2 && t0.elapsed() >= Duration::from_millis(950), "connections {} after {:?}", srv.connections(), t0.elapsed());
    Ok(format!("2 attempts, waited {} ms for Retry-After: 1 via the host's sleeper (thread_sleep: one thread per sleep)", t0.elapsed().as_millis()))
}
async fn x5(t: T) -> Result<String, String> {
    let srv = MockServer::start(vec![Scenario::fixed(429, &[("Retry-After", "7")], E429), Scenario::sse(MIDERR.as_bytes(), 4096)]);
    let cb = model_callback(Arc::new(or(&srv)), t);
    let e429 = cb(req("hi")).await.err().ok_or("expected Err")?;
    let emid = cb(req("hi")).await.err().ok_or("expected Err")?;
    ensure!(!e429.contains('7'), "Retry-After survived the flattening?? {e429:?}");
    ensure!(!emid.contains("partial"), "partial text survived the flattening?? {emid:?}");
    Ok(format!("callback Err(String) for 429+Retry-After 7: {e429:?}; for mid-stream error after \"partial \": {emid:?} (retry hint, status, partial text, metadata all gone)"))
}
