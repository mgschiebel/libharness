//! SSE transcripts hand-built from the documented formats and OpenRouter's OpenAPI spec
//! (`ChatStreamChunk`, `ChatStreamToolCall`, `ChatUsage`). NOT captured from the live API.
#![allow(dead_code)]

/// Case 1 + 4 + 5: comments (also between events, CRLF endings), multi-byte text, a duplicated
/// `finish_reason` chunk, a usage chunk with `cost`/`cost_details`/`is_byok`, a served model that
/// differs from the requested `openai/gpt-4o`.
pub const TEXT: &str = concat!(
    ": OPENROUTER PROCESSING\n\n",
    "data: {\"id\":\"gen-1\",\"object\":\"chat.completion.chunk\",\"model\":\"anthropic/claude-x\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"h\u{e9}llo \"},\"finish_reason\":null}]}\n\n",
    ": OPENROUTER PROCESSING\n\n",
    "data: {\"id\":\"gen-1\",\"object\":\"chat.completion.chunk\",\"model\":\"anthropic/claude-x\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"w\u{f6}rld \u{1F600}\"},\"finish_reason\":null}]}\r\n\r\n",
    ": keep-alive\r\n\r\n",
    "data: {\"id\":\"gen-1\",\"object\":\"chat.completion.chunk\",\"model\":\"anthropic/claude-x\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"stop\",\"native_finish_reason\":\"stop\"}]}\n\n",
    "data: {\"id\":\"gen-1\",\"object\":\"chat.completion.chunk\",\"model\":\"anthropic/claude-x\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"stop\",\"native_finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":4,\"total_tokens\":9,\"cost\":0.0001,\"cost_details\":{\"upstream_inference_cost\":0.00009,\"upstream_inference_prompt_cost\":0.00004,\"upstream_inference_completions_cost\":0.00005},\"is_byok\":true,\"prompt_tokens_details\":{\"cached_tokens\":2},\"completion_tokens_details\":{\"reasoning_tokens\":1}}}\n\n",
    "data: [DONE]\n\n"
);

pub fn tc(index: usize, id: Option<&str>, name: Option<&str>, args: &str) -> String {
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

/// Case 3: two parallel calls interleaved by `index`; `id`/`name` only on each call's first
/// fragment; argument fragments split mid-token and mid-UTF-8-scalar level (the `\u{e9}` is
/// whole inside a fragment, the chunk sizes used by the tests cut the bytes anywhere).
pub fn tools_stream() -> String {
    let mut s = String::from(": OPENROUTER PROCESSING\n\n");
    s += &tc(0, Some("call_a"), Some("add"), "");
    s += &tc(1, Some("call_b"), Some("echo"), "{\"te");
    s += &tc(0, None, None, "{\"a\": 1, ");
    s += &tc(0, None, None, "\"b\": 2}");
    s += &tc(1, None, None, "xt\": \"h\u{e9}\"}");
    s += "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}}\n\ndata: [DONE]\n\n";
    s
}

/// Case 2: partial text, a comment, then an error chunk inside HTTP 200 (top-level `error` plus a
/// choice with `finish_reason: error`), then a stray chunk that must be ignored.
pub const MIDERR: &str = concat!(
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial \"},\"finish_reason\":null}]}\n\n",
    ": OPENROUTER PROCESSING\n\n",
    "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"error\":{\"code\":429,\"message\":\"Rate limit exceeded\",\"metadata\":{\"error_type\":\"rate_limit_exceeded\",\"provider_code\":\"rl-1\"}},\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"error\"}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ignored after error\"},\"finish_reason\":null}]}\n\n"
);

pub const SIZES: [usize; 7] = [1, 2, 3, 5, 17, 64, 100_000];
