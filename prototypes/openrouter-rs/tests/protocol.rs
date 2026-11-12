//! Cases 1-8 of the shared protocol, run through the adapter's session-task callback shape
//! (and its structured `call`) against the loopback mock. Fixtures are hand-built, not live.
mod common;
use common::*;
use openrouter_rs_adapter_proto::*;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

const SIZES: [usize; 5] = [1, 3, 17, 64, 100_000];

fn adapter(mock: &Mock) -> Arc<OpenRouterAdapter> {
    Arc::new(OpenRouterAdapter::new(&mock.base_url(), "sk-test", REQUESTED).unwrap())
}
fn hi() -> ModelRequest {
    ModelRequest {
        messages: vec![Message::User("hi".into())],
        ..Default::default()
    }
}
async fn call(replies: Vec<Reply>, req: ModelRequest) -> (Result<ModelReply, ModelError>, Mock) {
    let mock = Mock::start(replies);
    let a = adapter(&mock);
    let r = a.call(&req, &|_| {}).await;
    (r, mock)
}

// ---- case 1: SSE comment lines, also between events (and inside one), CRLF, split UTF-8 --------
#[tokio::test]
async fn case1_comment_lines_at_every_chunk_size() {
    for sz in SIZES {
        let (r, _) = call(vec![Reply::sse(text_stream(), sz)], hi()).await;
        let r = r.unwrap_or_else(|e| panic!("chunk size {sz}: {e}"));
        assert_eq!(r.text, "h\u{e9}llo w\u{f6}rld \u{1F600}", "chunk size {sz}");
        assert_eq!(r.finish, FinishReason::Stop);
    }
}

// ---- case 2: error mid-stream inside an HTTP 200 -----------------------------------------------
#[tokio::test]
async fn case2a_documented_shape_error_plus_choice_loses_the_error_object() {
    // OpenRouter's documented shape: top-level `error` AND a choice with finish_reason "error".
    // The crate decodes it as an ordinary chunk and drops the top-level `error`; the only signal is
    // `finish_reason == "error"` on the terminal event. Code, message and metadata are gone.
    let (r, _) = call(vec![Reply::sse(mid_error_stream(false), 100_000)], hi()).await;
    let e = r.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Upstream);
    assert_eq!(
        e.code, None,
        "429 / rate_limit_exceeded never reaches the adapter"
    );
    assert_eq!(e.metadata, None);
    assert!(e.message.contains("error object not exposed"));
    assert_eq!(
        e.partial.unwrap().text,
        "partial ",
        "the partial text is recoverable (adapter accumulates it)"
    );
}
#[tokio::test]
async fn case2a_text_after_the_error_chunk_is_indistinguishable_from_partial_text() {
    // The unified stream has no per-chunk finish signal, so a chunk sent after the error chunk
    // (OpenRouter should not do this) is folded into the partial text.
    let (r, _) = call(vec![Reply::sse(mid_error_stream(true), 100_000)], hi()).await;
    assert_eq!(
        r.unwrap_err().partial.unwrap().text,
        "partial ignored after error"
    );
}
#[tokio::test]
async fn case2b_error_object_alone_in_a_frame_surfaces_as_api_error() {
    for sz in SIZES {
        let (r, _) = call(vec![Reply::sse(bare_error_stream(), sz)], hi()).await;
        let e = r.unwrap_err();
        assert_eq!(
            (e.kind, e.code, e.status),
            (ErrorKind::Upstream, Some(502), Some(502)),
            "size {sz}"
        );
        assert_eq!(e.message, "upstream down");
        assert_eq!(
            e.metadata.as_ref().unwrap()["error_type"],
            "provider_overloaded"
        );
        assert_eq!(
            e.partial.unwrap().text,
            "partial ",
            "the adapter stops at the first Error event"
        );
    }
}
#[tokio::test]
async fn case2c_error_chunk_with_envelope_but_no_choices_is_silently_empty() {
    // Same top-level `error`, but the chunk carries the envelope and `choices: []`: it decodes as a
    // normal empty chunk, no finish_reason arrives, and only the adapter's own check turns it into an error.
    let mut s = data(&chunk(
        json!({"content":"partial "}),
        Value::Null,
        json!({}),
    ));
    let mut c = chunk(
        json!({}),
        Value::Null,
        json!({"error":{"code":500,"message":"boom"}}),
    );
    c["choices"] = json!([]);
    s += &data(&c);
    let (r, _) = call(vec![Reply::sse(s, 100_000)], hi()).await;
    let e = r.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Truncated);
    assert_eq!(e.code, None);
    assert_eq!(e.partial.unwrap().text, "partial ");
}

// ---- case 3: parallel tool calls, interleaved by index, fragments split mid-token ----------------
#[tokio::test]
async fn case3_parallel_tool_calls_interleaved_and_split() {
    for sz in SIZES {
        let (r, _) = call(vec![Reply::sse(tools_stream(), sz)], hi()).await;
        let r = r.unwrap_or_else(|e| panic!("chunk size {sz}: {e}"));
        assert_eq!(r.finish, FinishReason::ToolCalls);
        assert_eq!(r.tool_calls.len(), 2, "size {sz}");
        assert_eq!(
            (r.tool_calls[0].id.as_str(), r.tool_calls[0].name.as_str()),
            ("call_a", "add")
        );
        assert_eq!(r.tool_calls[0].args, json!({"a": 1, "b": 2}));
        assert_eq!(
            (r.tool_calls[1].id.as_str(), r.tool_calls[1].name.as_str()),
            ("call_b", "echo")
        );
        assert_eq!(r.tool_calls[1].args, json!({"text": "h\u{e9}"}));
    }
}
#[tokio::test]
async fn case3_id_and_name_arriving_in_later_fragments_are_merged() {
    let mut s = tool_fragment(0, None, None, "{\"a\":");
    s += &tool_fragment(0, Some("late_id"), None, " 1}");
    s += &tool_fragment(0, None, Some("late_name"), "");
    s += &data(&chunk(json!({}), json!("tool_calls"), json!({})));
    let (r, _) = call(vec![Reply::sse(s, 100_000)], hi()).await;
    let r = r.unwrap();
    assert_eq!(
        (r.tool_calls[0].id.as_str(), r.tool_calls[0].name.as_str()),
        ("late_id", "late_name")
    );
    assert_eq!(r.tool_calls[0].args, json!({"a": 1}));
}
#[tokio::test]
async fn case3_tool_call_that_never_had_an_id_is_dropped_by_the_crate_and_caught_by_the_adapter() {
    let mut s = tool_fragment(0, Some("c0"), Some("ok"), "{}");
    s += &tool_fragment(1, None, Some("noid"), "{\"x\":1}"); // no id on any fragment
    s += &data(&chunk(json!({}), json!("tool_calls"), json!({})));
    let (r, _) = call(vec![Reply::sse(s, 100_000)], hi()).await;
    let e = r.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Protocol);
    assert_eq!(
        e.partial.unwrap().tool_calls.len(),
        1,
        "the crate returned only the call that had id and name"
    );
}
#[tokio::test]
async fn case3_malformed_and_empty_arguments() {
    let mut s = tool_fragment(0, Some("c0"), Some("bad"), "{\"a\": ");
    s += &tool_fragment(1, Some("c1"), Some("noargs"), "");
    s += &data(&chunk(json!({}), json!("tool_calls"), json!({})));
    let (r, _) = call(vec![Reply::sse(s, 7)], hi()).await;
    let r = r.unwrap();
    assert!(r.tool_calls[0].args_error.is_some());
    assert_eq!(r.tool_calls[1].args, json!({}));
}

// ---- case 4: usage chunk with cost, cost_details, is_byok and a duplicated finish_reason ---------
#[tokio::test]
async fn case4_usage_cost_details_byok_and_duplicate_finish() {
    let (r, _) = call(vec![Reply::sse(text_stream(), 100_000)], hi()).await;
    let r = r.unwrap();
    assert_eq!(r.finish, FinishReason::Stop);
    let u = r.usage.unwrap();
    assert_eq!(
        (u.prompt_tokens, u.completion_tokens, u.total_tokens),
        (5, 4, 9)
    );
    assert_eq!(u.cost, Some(0.0001));
    assert_eq!(u.is_byok, Some(true));
    let cd = u.cost_details.unwrap();
    assert_eq!(cd["upstream_inference_cost"], 0.00008);
    assert_eq!(cd["upstream_inference_prompt_cost"], 0.00002);
    assert_eq!(cd["upstream_inference_completions_cost"], 0.00006);
}
#[tokio::test]
async fn case4_usage_on_a_chunk_with_empty_choices() {
    let mut s = data(&chunk(json!({"content":"x"}), json!("stop"), json!({})));
    let mut u = chunk(
        json!({}),
        Value::Null,
        json!({"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5,"cost":0.5}}),
    );
    u["choices"] = json!([]);
    s += &data(&u);
    s += "data: [DONE]\n\n";
    let (r, _) = call(vec![Reply::sse(s, 100_000)], hi()).await;
    let r = r.unwrap();
    assert_eq!((r.tokens(), r.usage.unwrap().cost), (5, Some(0.5)));
}

// ---- case 5: served model differs from the requested one ----------------------------------------
#[tokio::test]
async fn case5_served_model_is_reported_and_differs_from_requested() {
    let (r, mock) = call(vec![Reply::sse(text_stream(), 100_000)], hi()).await;
    assert_eq!(r.unwrap().model.as_deref(), Some(SERVED));
    assert_eq!(mock.requests.lock().unwrap()[0].json()["model"], REQUESTED);
}

// ---- case 6: dropping the call future closes the server-side socket ----------------------------
#[tokio::test]
async fn case6_dropping_the_call_future_closes_the_socket() {
    let mock = Mock::start(vec![Reply::sse(
        text_stream()
            .split("data: [DONE]")
            .next()
            .unwrap()
            .to_string(),
        100_000,
    )
    .hold()]);
    let cb = model_callback(adapter(&mock));
    let mut fut = cb(hi());
    tokio::select! {
        _ = &mut fut => panic!("the call must still be waiting on the held stream"),
        _ = tokio::time::sleep(Duration::from_millis(500)) => {}
    }
    assert!(mock.wait_accepted(Duration::from_secs(1)));
    assert_eq!(
        mock.peer_closed.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "still open before the drop"
    );
    drop(fut);
    // The close is done by reqwest's spawned connection task, not synchronously by the drop, so the
    // runtime must keep running: wait with an async sleep (a blocking wait would starve that task).
    let t = std::time::Instant::now();
    while t.elapsed() < Duration::from_secs(5)
        && mock.peer_closed.load(std::sync::atomic::Ordering::SeqCst) == 0
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        mock.peer_closed.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "server saw the client close the connection"
    );
}

// ---- case 7: models, provider, reasoning, plugins reach the wire --------------------------------
fn options() -> Value {
    json!({
        "models": ["anthropic/claude-3.5-sonnet", "openai/gpt-4o-mini"],
        "provider": {"order": ["anthropic", "openai"], "allow_fallbacks": false, "require_parameters": true, "data_collection": "deny", "zdr": true, "sort": "latency"},
        "reasoning": {"effort": "low", "exclude": true},
        "plugins": [{"id": "web", "max_results": 2}, {"id": "response-healing"}]
    })
}
#[tokio::test]
async fn case7_models_provider_reasoning_plugins_reach_the_wire_as_typed_fields() {
    let req = ModelRequest {
        extra: Some(options()),
        ..hi()
    };
    let (r, mock) = call(vec![Reply::sse(text_stream(), 100_000)], req).await;
    r.unwrap();
    let rec = mock.requests.lock().unwrap()[0].clone();
    assert_eq!(
        (rec.method.as_str(), rec.path.as_str()),
        ("POST", "/api/v1/chat/completions")
    );
    assert_eq!(rec.header("authorization"), Some("Bearer sk-test"));
    let body = rec.json();
    let want = options();
    for k in ["models", "provider", "reasoning", "plugins"] {
        assert_eq!(body[k], want[k], "field {k} on the wire");
    }
    assert_eq!(body["stream"], true);
    assert_eq!(body["model"], REQUESTED);
}
#[tokio::test]
async fn case7_a_typo_in_a_typed_field_is_rejected_not_silently_ignored() {
    let mock = Mock::start(vec![Reply::sse(text_stream(), 100)]);
    let req = ModelRequest {
        extra: Some(json!({"provider": {"ordre": ["x"]}})),
        ..hi()
    };
    let e = adapter(&mock).call(&req, &|_| {}).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Config);
    assert!(e.message.contains("unknown field `ordre`"), "{}", e.message);
    assert!(mock.requests.lock().unwrap().is_empty(), "nothing was sent");
}
#[tokio::test]
async fn case7_a_field_the_crate_does_not_type_has_no_way_onto_the_wire() {
    // `deferred_tools`-style future fields: the request type has no flattened `extra`, so the adapter
    // has nowhere to put an unknown top-level field. There is no JSON escape hatch.
    let req = ModelRequest {
        extra: Some(json!({"some_future_field": {"a": 1}})),
        ..hi()
    };
    let (r, mock) = call(vec![Reply::sse(text_stream(), 100_000)], req).await;
    r.unwrap();
    assert!(mock.requests.lock().unwrap()[0]
        .json()
        .get("some_future_field")
        .is_none());
}
#[tokio::test]
async fn case7_conversation_and_tool_encoding() {
    let req = ModelRequest {
        messages: vec![
            Message::User("add".into()),
            Message::Assistant {
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "c1".into(),
                    name: "add".into(),
                    args: json!({"a": 1}),
                    args_error: None,
                }],
            },
            Message::ToolResult {
                call_id: "c1".into(),
                name: "add".into(),
                content: "3".into(),
                is_error: true,
            },
        ],
        tools: vec![ToolSpec {
            name: "add".into(),
            description: "adds".into(),
            params: json!({"type": "object"}),
        }],
        extra: None,
    };
    let (r, mock) = call(vec![Reply::sse(text_stream(), 100_000)], req).await;
    r.unwrap();
    let rec = mock.requests.lock().unwrap()[0].clone();
    let b = rec.json();
    assert_eq!(b["messages"][1]["content"], Value::Null);
    assert_eq!(
        b["messages"][1]["tool_calls"][0]["function"]["arguments"],
        "{\"a\":1}"
    );
    assert_eq!(b["messages"][1]["tool_calls"][0]["type"], "function");
    assert_eq!(b["messages"][2]["role"], "tool");
    assert_eq!(b["messages"][2]["tool_call_id"], "c1");
    assert!(
        b["messages"][2].get("is_error").is_none(),
        "is_error has no slot on a tool message"
    );
    assert_eq!(b["tools"][0]["function"]["name"], "add");
    // headers the crate adds on its own
    println!("request headers: {:?}", rec.headers);
}

// ---- case 8: HTTP 429 with Retry-After, 402, as plain error responses ----------------------------
#[tokio::test]
async fn case8_http_429_retry_after_and_402() {
    let body429 = r#"{"error":{"code":429,"message":"slow down","metadata":{"error_type":"rate_limit_exceeded"}}}"#;
    let (r, _) = call(
        vec![Reply::http(429, &[("Retry-After", "7")], body429)],
        hi(),
    )
    .await;
    let e = r.unwrap_err();
    assert_eq!(
        (e.kind, e.status, e.retry_after_secs, e.retryable()),
        (ErrorKind::RateLimited, Some(429), Some(7), true)
    );
    assert_eq!(e.metadata.unwrap()["error_type"], "rate_limit_exceeded");

    let (r, _) = call(
        vec![Reply::http(
            402,
            &[],
            r#"{"error":{"code":402,"message":"no credits"}}"#,
        )],
        hi(),
    )
    .await;
    let e = r.unwrap_err();
    assert_eq!(
        (e.kind, e.status, e.retryable(), e.message.as_str()),
        (ErrorKind::Credits, Some(402), false, "no credits")
    );
}
#[tokio::test]
async fn case8_retry_after_http_date_form_is_not_parsed() {
    let (r, _) = call(
        vec![Reply::http(
            429,
            &[("Retry-After", "Wed, 21 Oct 2026 07:28:00 GMT")],
            r#"{"error":{"code":429,"message":"x"}}"#,
        )],
        hi(),
    )
    .await;
    assert_eq!(r.unwrap_err().retry_after_secs, None);
}
#[tokio::test]
async fn case8_non_json_503_body() {
    let (r, _) = call(
        vec![Reply::http(503, &[], "<html>bad gateway</html>")],
        hi(),
    )
    .await;
    let e = r.unwrap_err();
    println!("503 html body -> {e:?}");
    assert_eq!(e.status, Some(503));
}

// ---- extras the ticket's uncertainties asked about ----------------------------------------------
#[tokio::test]
async fn extra_http_200_with_plain_json_error_body_is_not_an_error_to_the_crate() {
    // A non-SSE body has no `data:` lines, so the crate sees an empty stream. Only the adapter's
    // "no finish reason" check turns it into an error; the error object is lost.
    let (r, _) = call(
        vec![Reply::http(
            200,
            &[],
            r#"{"error":{"code":402,"message":"out of credit"}}"#,
        )],
        hi(),
    )
    .await;
    let e = r.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Truncated);
    assert_eq!(e.code, None);
}
#[tokio::test]
async fn extra_connection_cut_mid_stream() {
    let body = data(&chunk(json!({"content":"abc"}), Value::Null, json!({})));
    let (r, _) = call(vec![Reply::sse(body, 100_000).abrupt()], hi()).await;
    let e = r.unwrap_err();
    println!("abrupt close -> {e:?}");
    assert_eq!(e.partial.unwrap().text, "abc");
}
#[tokio::test]
async fn extra_chunk_missing_envelope_fields_is_a_decode_error() {
    // The crate's chunk type requires id, created, model and object on every chunk.
    let s = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n";
    let (r, _) = call(vec![Reply::sse(s, 100_000)], hi()).await;
    let e = r.unwrap_err();
    println!("chunk without envelope -> {e:?}");
    assert_eq!(e.kind, ErrorKind::Protocol);
}
#[tokio::test]
async fn extra_callback_flattens_the_structured_error_to_a_string() {
    let mock = Mock::start(vec![Reply::http(
        429,
        &[("Retry-After", "7")],
        r#"{"error":{"code":429,"message":"slow down"}}"#,
    )]);
    let cb = model_callback(adapter(&mock));
    let e = cb(hi()).await.unwrap_err();
    assert_eq!(e, "RateLimited: slow down");
}
#[tokio::test]
async fn extra_eof_inside_an_event_panics_the_crate_and_the_adapter_catches_it() {
    let c = chunk(json!({"content":"tail"}), json!("stop"), json!({}));
    let body =
        data(&chunk(json!({"content":"abc"}), Value::Null, json!({}))) + &format!("data: {c}\n"); // no blank line, then EOF
    let (r, _) = call(vec![Reply::sse(body, 100_000)], hi()).await;
    let e = r.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Protocol);
    assert!(e.message.contains("panicked"));
    assert_eq!(
        e.partial.unwrap().text,
        "abctail",
        "text before the panic is kept"
    );
}
