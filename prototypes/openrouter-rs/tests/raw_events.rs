//! What the crate's unified stream itself yields (no adapter), for the ticket's open uncertainties.
mod common;
use common::*;
use futures_util::StreamExt;
use openrouter_rs::error::OpenRouterError;
use openrouter_rs::types::completion::Choice;
use openrouter_rs::types::stream::UnifiedStreamEvent as E;
use openrouter_rs::{
    api::chat::{ChatCompletionRequest, Message},
    types::Role,
    OpenRouterClient,
};
use serde_json::{json, Value};

fn client(mock: &Mock) -> OpenRouterClient {
    OpenRouterClient::builder()
        .base_url(mock.base_url())
        .api_key("k")
        .build()
        .unwrap()
}
fn req() -> ChatCompletionRequest {
    ChatCompletionRequest::builder()
        .model("m")
        .messages(vec![Message::new(Role::User, "hi")])
        .build()
        .unwrap()
}
async fn events(body: String) -> Vec<E> {
    let mock = Mock::start(vec![Reply::sse(body, 100_000)]);
    client(&mock)
        .chat()
        .stream_unified(&req())
        .await
        .unwrap()
        .collect()
        .await
}

#[tokio::test]
async fn error_plus_choices_chunk_is_a_normal_chunk_and_the_error_object_is_dropped() {
    let ev = events(mid_error_stream(true)).await;
    assert!(
        !ev.iter().any(|e| matches!(e, E::Error(_))),
        "no Error event: {ev:?}"
    );
    // content after the error-bearing chunk still flows
    assert!(matches!(&ev[1], E::ContentDelta(t) if t == "ignored after error"));
    match ev.last().unwrap() {
        E::Done {
            finish_reason,
            usage,
            ..
        } => assert_eq!(
            (finish_reason.as_deref(), usage.is_none()),
            (Some("error"), true)
        ),
        other => panic!("{other:?}"),
    }
}
#[tokio::test]
async fn the_non_unified_stream_has_no_place_for_the_error_object_either() {
    let mock = Mock::start(vec![Reply::sse(mid_error_stream(false), 100_000)]);
    let chunks: Vec<_> = client(&mock)
        .chat()
        .stream(&req())
        .await
        .unwrap()
        .collect()
        .await;
    let last = chunks.last().unwrap().as_ref().unwrap();
    // `CompletionsResponse` has no top-level `error` field; the per-choice `error` is empty
    assert!(matches!(&last.choices[0], Choice::Streaming(c) if c.error.is_none()));
    assert!(serde_json::to_value(last).unwrap().get("error").is_none());
}
#[tokio::test]
async fn error_object_alone_is_an_api_error_event_and_the_stream_keeps_going() {
    let ev = events(bare_error_stream()).await;
    assert!(
        matches!(&ev[1], E::Error(OpenRouterError::Api(c)) if c.api_code == Some(502) && c.status.as_u16() == 502)
    );
    assert!(
        matches!(&ev[2], E::ContentDelta(t) if t == "ignored after error"),
        "the crate does not stop at an error event: {ev:?}"
    );
    assert!(matches!(
        ev.last().unwrap(),
        E::Done {
            finish_reason: None,
            ..
        }
    ));
}
#[tokio::test]
async fn tool_call_without_an_id_or_name_is_silently_dropped_from_done() {
    let mut s = tool_fragment(0, Some("c0"), Some("ok"), "{}");
    s += &tool_fragment(1, None, Some("noid"), "{\"x\":1}");
    s += &tool_fragment(2, Some("noname"), None, "{\"y\":1}");
    s += &data(&chunk(json!({}), json!("tool_calls"), json!({})));
    let ev = events(s).await;
    assert_eq!(
        ev.iter().filter(|e| matches!(e, E::ToolDelta(_))).count(),
        3,
        "all three fragments were announced"
    );
    match ev.last().unwrap() {
        E::Done {
            tool_calls,
            finish_reason,
            ..
        } => {
            assert_eq!(finish_reason.as_deref(), Some("tool_calls"));
            assert_eq!(
                tool_calls.len(),
                1,
                "0.23.0 still drops them without a trace: {tool_calls:?}"
            );
        }
        other => panic!("{other:?}"),
    }
}
#[tokio::test]
async fn fields_the_terminal_event_does_not_carry() {
    // provider, native_finish_reason, system_fingerprint, service_tier, openrouter_metadata are decoded by the
    // chunk type but the unified terminal event exposes only id, model, finish_reason, tool_calls, usage.
    let mut s = data(&chunk(
        json!({"content":"x"}),
        Value::Null,
        json!({"provider":"Anthropic","system_fingerprint":"fp","service_tier":"flex","openrouter_metadata":{"a":1}}),
    ));
    s += &data(&chunk(json!({}), json!("stop"), json!({})));
    let ev = events(s).await;
    let dbg = format!("{:?}", ev.last().unwrap());
    for lost in ["Anthropic", "fp", "flex", "openrouter_metadata"] {
        assert!(!dbg.contains(lost), "{lost} unexpectedly visible: {dbg}");
    }
    // reasoning reaches events, so it can be kept if the adapter chooses to
    let mut s = data(&chunk(json!({"reasoning":"think"}), Value::Null, json!({})));
    s += &data(&chunk(
        json!({"reasoning_details":[{"type":"reasoning.text","text":"t","index":0}]}),
        Value::Null,
        json!({}),
    ));
    s += &data(&chunk(json!({}), json!("stop"), json!({})));
    let ev = events(s).await;
    assert!(ev
        .iter()
        .any(|e| matches!(e, E::ReasoningDelta(t) if t == "think")));
    assert!(ev.iter().any(|e| matches!(e, E::ReasoningDetailsDelta(_))));
}
/// Run the unified stream to the end on its own task and report whether it panicked.
async fn run_to_end(body: &str) -> Result<Vec<String>, String> {
    let mock = Mock::start(vec![Reply::sse(body.to_string(), 100_000)]);
    let c = client(&mock);
    let h = tokio::spawn(async move {
        let mut s = c.chat().stream_unified(&req()).await.unwrap();
        let mut out = vec![];
        while let Some(e) = s.next().await {
            out.push(format!("{e:?}"));
        }
        out
    });
    match h.await {
        Ok(v) => Ok(v),
        Err(e) if e.is_panic() => {
            let p = e.into_panic();
            Err(p
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_default())
        }
        Err(e) => Err(e.to_string()),
    }
}
#[tokio::test]
async fn eof_inside_an_event_makes_the_crates_stream_panic() {
    // The WHATWG rule is to discard an incomplete event at end of stream. The crate instead dispatches it
    // and then polls its exhausted line stream again, which panics (futures-util `Unfold` guard).
    let c = chunk(json!({"content":"tail"}), json!("stop"), json!({}));
    for (name, body) in [
        ("data line, newline, EOF", format!("data: {c}\n")),
        ("data line, no newline, EOF", format!("data: {c}")),
    ] {
        let r = run_to_end(&body).await;
        println!("{name} -> {r:?}");
        assert!(
            matches!(&r, Err(m) if m.contains("Unfold must not be polled")),
            "{name}: {r:?}"
        );
    }
    // with the blank line the same bytes are fine
    let ok = run_to_end(&format!("data: {c}\n\n")).await.unwrap();
    assert!(matches!(ok.last().map(String::as_str), Some(d) if d.starts_with("Done")));
}
#[tokio::test]
async fn bare_cr_is_not_a_line_ending() {
    let c = chunk(json!({"content":"cr"}), json!("stop"), json!({}));
    let r = run_to_end(&format!("data: {c}\r\r")).await;
    println!("bare-CR stream -> {r:?}");
    // a lone CR is not a line break for the crate's line reader (WHATWG says it is); the whole body is one
    // unterminated line, so this ends in the same panic as an EOF inside an event
    assert!(
        matches!(&r, Err(m) if m.contains("Unfold must not be polled")),
        "{r:?}"
    );
}
