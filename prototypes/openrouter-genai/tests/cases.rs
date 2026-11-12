//! Cases 1-8 of the shared protocol, through genai's chat-stream call pointed at a loopback mock.
//! Each test states the verdict it observed; the README table is the summary.
mod fixtures;
mod support;
use fixtures::*;
use openrouter_genai::*;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use support::*;

fn adapter(m: &Mock) -> Arc<OpenRouterGenai> {
    Arc::new(OpenRouterGenai::new(Config { base_url: m.base_url(), api_key: "sk-test".into(), model: "openai/gpt-4o".into() }))
}
fn hi() -> ModelRequest {
    ModelRequest { messages: vec![Message::User("hi".into())], ..Default::default() }
}
async fn call(script: Script) -> Result<ModelReply, ModelError> {
    let m = Mock::start(script);
    adapter(&m).complete(hi()).await
}

// -- Case 1: SSE comment lines, also between events (and CRLF, split UTF-8) --------------------
#[tokio::test(flavor = "multi_thread")]
async fn case1_comments_between_events_at_every_chunk_size() {
    for sz in SIZES {
        let r = call(Script::sse(TEXT, sz)).await.unwrap_or_else(|e| panic!("size {sz}: {e}"));
        assert_eq!(r.text, "h\u{e9}llo w\u{f6}rld \u{1F600}", "size {sz}");
        assert_eq!(r.finish, FinishReason::Stop);
    }
}

// -- Case 2: error chunk mid-stream inside an HTTP 200, with text already delivered -------------
#[tokio::test(flavor = "multi_thread")]
async fn case2_mid_stream_error_keeps_partial_text() {
    for sz in SIZES {
        let e = call(Script::sse(MIDERR, sz)).await.unwrap_err();
        assert_eq!(e.kind, ErrorKind::RateLimited, "size {sz}");
        assert_eq!(e.code, Some(429));
        assert!(e.retryable());
        // Recovered by the adapter accumulating `Chunk` events before genai's error item arrives.
        assert_eq!(e.partial.as_ref().unwrap().text, "partial ", "later content must not leak in");
        assert_eq!(e.metadata.as_ref().unwrap()["error_type"], "rate_limit_exceeded");
        assert_eq!(e.metadata.as_ref().unwrap()["provider_code"], "rl-1");
    }
}
#[tokio::test(flavor = "multi_thread")]
async fn case2_error_chunk_as_the_only_event() {
    let s = "data: {\"error\":{\"code\":502,\"message\":\"upstream down\"},\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"error\"}]}\n\n";
    let e = call(Script::sse(s, 4)).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Upstream);
    assert!(e.partial.unwrap().text.is_empty());
}
#[tokio::test(flavor = "multi_thread")]
async fn case2_finish_reason_error_without_error_object() {
    // genai ends normally with StopReason::Other("error"); the adapter has to turn that into a failure.
    let s = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"error\"}]}\n\ndata: [DONE]\n\n";
    assert_eq!(call(Script::sse(s, 9)).await.unwrap_err().kind, ErrorKind::Upstream);
}
#[tokio::test(flavor = "multi_thread")]
async fn case2_truncated_streams() {
    let cut = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"abc\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"index\":0,\"del";
    let e = call(Script::sse(cut, 6)).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Protocol);
    assert_eq!(e.partial.unwrap().text, "abc");
    // Clean EOF with no finish and no [DONE]: genai ends the stream silently, no End event.
    let clean = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"abc\"},\"finish_reason\":null}]}\n\n";
    let e = call(Script::sse(clean, 6)).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Truncated);
    assert_eq!(e.partial.unwrap().text, "abc");
}
/// OpenRouter documents that a 200 whose body is only an error is a failure. genai gives the adapter
/// nothing to react to: the stream just ends after `Start`.
#[tokio::test(flavor = "multi_thread")]
async fn case2_http_200_with_plain_json_error_body_is_not_recognised() {
    let e = call(Script::http(200, &[("Content-Type", "application/json")], "{\"error\":{\"code\":402,\"message\":\"out of credit\"}}")).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Truncated, "no 402/Credits recognition: the message is lost");
    assert!(!e.message.contains("credit"));
}
/// Documents raw genai behaviour the adapter has to work around: after the error item the stream keeps yielding.
#[tokio::test(flavor = "multi_thread")]
async fn case2_genai_keeps_yielding_after_the_error_item() {
    use futures_util::StreamExt;
    use genai::chat::*;
    let m = Mock::start(Script::sse(MIDERR, 100_000));
    let c = genai::Client::builder()
        .with_auth_resolver_fn(|_m| Ok(Some(genai::resolver::AuthData::from_single("k"))))
        .with_service_target_resolver_fn({
            let b = m.base_url();
            move |mut t: genai::ServiceTarget| {
                t.endpoint = genai::resolver::Endpoint::from_owned(b.clone());
                Ok(t)
            }
        })
        .build()
        .unwrap();
    let mut s = c.exec_chat_stream("open_router::openai/gpt-4o", ChatRequest::from_user("hi"), None).await.unwrap().stream;
    let mut seen = vec![];
    while let Some(ev) = s.next().await {
        seen.push(match ev {
            Ok(ChatStreamEvent::Chunk(c)) => format!("chunk:{}", c.content),
            Ok(e) => format!("{e:?}").split('(').next().unwrap().to_string(),
            Err(_) => "ERR".into(),
        });
    }
    assert_eq!(seen, ["Start", "chunk:partial ", "ERR", "chunk:ignored after error"]);
}

// -- Case 3: two parallel tool calls, interleaved by index, fragments split mid-token ----------
#[tokio::test(flavor = "multi_thread")]
async fn case3_interleaved_parallel_tool_calls() {
    let s = tools_stream();
    for sz in SIZES {
        let r = call(Script::sse(&s, sz)).await.unwrap_or_else(|e| panic!("size {sz}: {e}"));
        assert_eq!(r.finish, FinishReason::ToolCalls);
        assert_eq!(r.tool_calls.len(), 2, "size {sz}");
        assert_eq!((r.tool_calls[0].id.as_str(), r.tool_calls[0].name.as_str()), ("call_a", "add"));
        assert_eq!(r.tool_calls[0].args, json!({"a": 1, "b": 2}));
        assert_eq!((r.tool_calls[1].id.as_str(), r.tool_calls[1].name.as_str()), ("call_b", "echo"));
        assert_eq!(r.tool_calls[1].args, json!({"text": "h\u{e9}"}));
        assert!(r.tool_calls.iter().all(|c| c.args_error.is_none()));
    }
}
#[tokio::test(flavor = "multi_thread")]
async fn case3_malformed_and_empty_arguments_are_left_to_the_adapter() {
    let mut s = tc(0, Some("c0"), Some("bad"), "{\"a\": ");
    s += &tc(1, Some("c1"), Some("noargs"), "");
    s += "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n";
    let r = call(Script::sse(&s, 7)).await.unwrap();
    assert!(r.tool_calls[0].args_error.is_some(), "genai leaves unparsable arguments as a string");
    assert_eq!(r.tool_calls[1].args, json!({}));
}

// -- Case 4: usage chunk with cost, cost_details, is_byok and a duplicated finish_reason -------
#[tokio::test(flavor = "multi_thread")]
async fn case4_usage_chunk_with_cost() {
    let r = call(Script::sse(TEXT, 11)).await.unwrap();
    let u = r.usage.unwrap();
    assert_eq!((u.prompt_tokens, u.completion_tokens, u.total_tokens), (5, 4, 9), "tokens come through genai's typed Usage");
    assert_eq!(u.cost, Some(0.0001), "cost only through the raw-frame side channel");
    let meta = r.meta.unwrap();
    assert_eq!(meta["is_byok"], true);
    assert_eq!(meta["cost_details"]["upstream_inference_cost"], 0.00009);
    assert_eq!(r.finish, FinishReason::Stop, "duplicate finish_reason is harmless");
    assert_eq!(r.text, "h\u{e9}llo w\u{f6}rld \u{1F600}");
}
/// genai's own `Usage` has no place for cost: prove it from its serialised form.
#[tokio::test(flavor = "multi_thread")]
async fn case4_genai_usage_has_no_cost_field() {
    let u: genai::chat::Usage = serde_json::from_value(json!({"prompt_tokens":5,"completion_tokens":4,"total_tokens":9,"cost":0.0001,"is_byok":true})).unwrap();
    let back = serde_json::to_value(&u).unwrap();
    assert!(back.get("cost").is_none() && back.get("is_byok").is_none());
}

/// Streamed `reasoning_details` (open upstream PR in genai): not in any typed event, only in raw frames.
#[tokio::test(flavor = "multi_thread")]
async fn reasoning_details_survive_only_through_the_side_channel() {
    let body = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning\":\"think \",\"reasoning_details\":[{\"type\":\"reasoning.text\",\"text\":\"think \",\"index\":0}]},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ans\",\"reasoning_details\":[{\"type\":\"reasoning.encrypted\",\"data\":\"abc\",\"index\":1}]},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"
    );
    let r = call(Script::sse(body, 100_000)).await.unwrap();
    assert_eq!(r.text, "ans");
    let meta = r.meta.unwrap();
    assert_eq!(meta["reasoning"], "think ");
    assert_eq!(meta["reasoning_details"][1]["data"], "abc");
}

// -- Case 5: served model differs from the requested one ---------------------------------------
#[tokio::test(flavor = "multi_thread")]
async fn case5_served_model() {
    let m = Mock::start(Script::sse(TEXT, 50));
    let r = adapter(&m).complete(hi()).await.unwrap();
    assert_eq!(m.last_request().json()["model"], "openai/gpt-4o");
    assert_eq!(r.model.as_deref(), Some("anthropic/claude-x"), "recovered from raw frames");
}
/// genai's typed events never carry the served model: the End event's JSON has no trace of it.
#[tokio::test(flavor = "multi_thread")]
async fn case5_genai_typed_events_do_not_expose_the_served_model() {
    use futures_util::StreamExt;
    use genai::chat::*;
    let m = Mock::start(Script::sse(TEXT, 100_000));
    let c = genai::Client::builder()
        .with_auth_resolver_fn(|_m| Ok(Some(genai::resolver::AuthData::from_single("k"))))
        .with_service_target_resolver_fn({
            let b = m.base_url();
            move |mut t: genai::ServiceTarget| {
                t.endpoint = genai::resolver::Endpoint::from_owned(b.clone());
                Ok(t)
            }
        })
        .build()
        .unwrap();
    let o = ChatOptions::default().with_capture_usage(true).with_capture_content(true);
    let mut s = c.exec_chat_stream("open_router::openai/gpt-4o", ChatRequest::from_user("hi"), Some(&o)).await.unwrap().stream;
    let mut all = String::new();
    while let Some(ev) = s.next().await {
        all += &serde_json::to_string(&ev.unwrap()).unwrap();
    }
    assert!(all.contains("\"captured_usage\"") && !all.contains("claude-x"));
}

// -- Case 6: dropping the call future closes the server-side socket ----------------------------
async fn cancel_case(a_fn: impl FnOnce(&Mock) -> Arc<OpenRouterGenai>) {
    let script = Script::sse(TEXT.split("data: [DONE]").next().unwrap(), 50).hanging();
    let m = Mock::start(script);
    let a = a_fn(&m);
    let fut = a.complete(hi());
    // The server delivers its chunks and then hangs; give up after 400 ms, which drops the future.
    let r = tokio::time::timeout(Duration::from_millis(400), fut).await;
    assert!(r.is_err(), "the call must still be waiting when we drop it");
    assert!(m.chunks_sent.load(std::sync::atomic::Ordering::SeqCst) > 0);
    assert!(m.wait_closed(Duration::from_secs(5)), "server side must see the connection close after the drop");
}
#[tokio::test(flavor = "multi_thread")]
async fn case6_drop_closes_socket() {
    cancel_case(adapter).await;
}
#[tokio::test(flavor = "multi_thread")]
async fn case6_drop_closes_socket_with_adapter_owned_runtime() {
    // The adapter spawns the call on its own runtime; its JoinHandle wrapper must abort on drop.
    cancel_case(|m| Arc::new(OpenRouterGenai::new(Config { base_url: m.base_url(), api_key: "k".into(), model: "openai/gpt-4o".into() }).with_owned_runtime())).await;
}

/// Control for case 6: while the future is alive the server must NOT see the connection close.
#[tokio::test(flavor = "multi_thread")]
async fn case6_control_connection_stays_open_while_the_future_lives() {
    let m = Mock::start(Script::sse(TEXT.split("data: [DONE]").next().unwrap(), 50).hanging());
    let a = adapter(&m);
    let h = tokio::spawn(a.complete(hi()));
    assert!(m.wait_chunks(3, Duration::from_secs(3)));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!m.client_closed.load(std::sync::atomic::Ordering::SeqCst));
    h.abort();
    assert!(m.wait_closed(Duration::from_secs(5)));
}

// -- Case 7: models / provider / reasoning / plugins reach the wire ----------------------------
#[tokio::test(flavor = "multi_thread")]
async fn case7_openrouter_only_fields_on_the_wire() {
    let m = Mock::start(Script::sse(TEXT, 100_000));
    let extra = json!({
        "models": ["anthropic/claude-x", "openai/gpt-4o-mini"],
        "provider": {"order": ["anthropic"], "allow_fallbacks": false},
        "reasoning": {"effort": "low", "max_tokens": 128},
        "plugins": [{"id": "web", "max_results": 2}]
    });
    let req = ModelRequest { extra: Some(extra.clone()), ..hi() };
    adapter(&m).complete(req).await.unwrap();
    let r = m.last_request();
    let b = r.json();
    for k in ["models", "provider", "reasoning", "plugins"] {
        assert_eq!(b[k], extra[k], "{k} reaches the wire verbatim via extra_body");
    }
    assert_eq!(b["model"], "openai/gpt-4o", "genai always writes `model`");
    assert_eq!(b["stream"], true);
    assert_eq!(r.header("authorization"), Some("Bearer sk-test"));
    assert!(r.request_line.starts_with("POST /api/v1/chat/completions "), "{}", r.request_line);
}
/// extra_body is an unchecked merge: it can also overwrite fields the adapter relies on.
#[tokio::test(flavor = "multi_thread")]
async fn case7_extra_body_is_unchecked_and_can_override_model_and_stream() {
    let m = Mock::start(Script::sse(TEXT, 100_000));
    let req = ModelRequest { extra: Some(json!({"model": "evil/override", "stream": false, "reasonning": {"effort": "low"}})), ..hi() };
    let _ = adapter(&m).complete(req).await;
    let b = m.last_request().json();
    assert_eq!(b["model"], "evil/override");
    assert_eq!(b["stream"], false);
    assert!(b.get("reasonning").is_some(), "a typo is forwarded unchanged (OpenRouter would ignore it)");
}
/// genai's typed reasoning option writes OpenAI's `reasoning_effort`, not OpenRouter's `reasoning` object.
#[tokio::test(flavor = "multi_thread")]
async fn case7_typed_reasoning_effort_is_not_openrouters_reasoning_object() {
    use futures_util::StreamExt;
    use genai::chat::*;
    let m = Mock::start(Script::sse(TEXT, 100_000));
    let c = genai::Client::builder()
        .with_auth_resolver_fn(|_m| Ok(Some(genai::resolver::AuthData::from_single("k"))))
        .with_service_target_resolver_fn({
            let b = m.base_url();
            move |mut t: genai::ServiceTarget| {
                t.endpoint = genai::resolver::Endpoint::from_owned(b.clone());
                Ok(t)
            }
        })
        .build()
        .unwrap();
    let o = ChatOptions::default().with_reasoning_effort(ReasoningEffort::Low);
    let mut s = c.exec_chat_stream("open_router::openai/gpt-4o", ChatRequest::from_user("hi"), Some(&o)).await.unwrap().stream;
    while s.next().await.is_some() {}
    let b = m.last_request().json();
    assert_eq!(b["reasoning_effort"], "low");
    assert!(b.get("reasoning").is_none());
}

// -- Case 8: HTTP 429 with Retry-After and HTTP 402 as plain error responses -------------------
#[tokio::test(flavor = "multi_thread")]
async fn case8_http_errors() {
    let body = "{\"error\":{\"code\":429,\"message\":\"slow down\",\"metadata\":{\"error_type\":\"rate_limit_exceeded\"}}}";
    let e = call(Script::http(429, &[("Retry-After", "7"), ("Content-Type", "application/json")], body)).await.unwrap_err();
    assert_eq!((e.kind, e.retry_after_secs, e.retryable(), e.status), (ErrorKind::RateLimited, Some(7), true, Some(429)));
    assert_eq!(e.message, "slow down");
    assert_eq!(e.metadata.unwrap()["error_type"], "rate_limit_exceeded");
    let e = call(Script::http(402, &[("Content-Type", "application/json")], "{\"error\":{\"code\":402,\"message\":\"no credits\"}}")).await.unwrap_err();
    assert_eq!((e.kind, e.retryable(), e.status), (ErrorKind::Credits, false, Some(402)));
    let e = call(Script::http(503, &[], "<html>bad gateway</html>")).await.unwrap_err();
    assert_eq!((e.kind, e.status), (ErrorKind::Upstream, Some(503)));
}

// -- Request translation (not a numbered case, but part of "what has to be translated") -------
#[tokio::test(flavor = "multi_thread")]
async fn request_translation_tools_and_tool_results() {
    let m = Mock::start(Script::sse(TEXT, 100_000));
    let req = ModelRequest {
        messages: vec![
            Message::User("add".into()),
            Message::Assistant { text: String::new(), tool_calls: vec![ToolCall { id: "c1".into(), name: "add".into(), args: json!({"a": 1}), args_error: None }] },
            Message::ToolResult { call_id: "c1".into(), name: "add".into(), content: "3".into(), is_error: true },
        ],
        tools: vec![ToolSpec { name: "add".into(), description: "adds".into(), params: json!({"type": "object"}) }],
        extra: None,
    };
    adapter(&m).complete(req).await.unwrap();
    let b = m.last_request().json();
    println!("{}", serde_json::to_string_pretty(&b).unwrap());
    assert_eq!(b["messages"][1]["tool_calls"][0]["function"]["arguments"], "{\"a\":1}");
    assert_eq!(b["messages"][2]["role"], "tool");
    assert_eq!(b["messages"][2]["tool_call_id"], "c1");
    assert_eq!(b["tools"][0]["function"]["name"], "add");
    assert!(b["messages"][2].get("is_error").is_none(), "ToolResult.is_error has no slot");
}

// -- The callback shape and the error flattening ------------------------------------------------
#[tokio::test(flavor = "multi_thread")]
async fn callback_shape_and_flattened_error() {
    let m = Mock::start(Script::sse(MIDERR, 100_000));
    let cb = model_callback(adapter(&m));
    let e = cb(hi()).await.unwrap_err();
    assert_eq!(e, "RateLimited: Rate limit exceeded", "retryability, retry-after, partial text and metadata are all gone");
}
