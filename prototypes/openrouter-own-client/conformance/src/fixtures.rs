//! Fixtures are SSE transcripts built from the documented formats and OpenRouter's OpenAPI schemas
//! (ChatStreamChunk, ChatStreamToolCall, ChatUsage). NOT captured from the live API.
pub const TEXT: &str = concat!(
    ": OPENROUTER PROCESSING\n\n",
    "data: {\"id\":\"gen-1\",\"object\":\"chat.completion.chunk\",\"model\":\"openai/gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"h\u{e9}llo \"},\"finish_reason\":null}]}\n\n",
    ": OPENROUTER PROCESSING\n\n",
    "data: {\"id\":\"gen-1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"w\u{f6}rld \u{1F600}\"},\"finish_reason\":null}]}\r\n\r\n",
    ": OPENROUTER PROCESSING\n\n",
    "data: {\"id\":\"gen-1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"stop\",\"native_finish_reason\":\"stop\"}]}\n\n",
    "data: {\"id\":\"gen-1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"stop\",\"native_finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":4,\"total_tokens\":9,\"cost\":0.0001}}\n\n",
    "data: [DONE]\n\n"
);
pub const TEXT_DECODED: &str = "h\u{e9}llo w\u{f6}rld \u{1F600}";

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
/// Two parallel calls, interleaved by `index`, arguments split mid-token, id/name only on the first fragment.
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
pub const MIDERR: &str = concat!(
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial \"},\"finish_reason\":null}]}\n\n",
    ": OPENROUTER PROCESSING\n\n",
    "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"error\":{\"code\":429,\"message\":\"Rate limit exceeded\",\"metadata\":{\"error_type\":\"rate_limit_exceeded\"}},\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"error\"}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ignored after error\"},\"finish_reason\":null}]}\n\n"
);
/// Usage chunk after a finish chunk, repeating `finish_reason`, with `cost`, `cost_details`, `is_byok` and token details.
/// The served model (anthropic/...) differs from the requested one (openai/gpt-4o).
pub const USAGE: &str = concat!(
    "data: {\"id\":\"gen-9\",\"model\":\"anthropic/claude-sonnet-4.5\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"ok\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"gen-9\",\"model\":\"anthropic/claude-sonnet-4.5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"stop\"}]}\n\n",
    "data: {\"id\":\"gen-9\",\"model\":\"anthropic/claude-sonnet-4.5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"stop\"}],",
    "\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":20,\"total_tokens\":30,\"cost\":0.00123,\"is_byok\":true,",
    "\"cost_details\":{\"upstream_inference_cost\":0.001,\"upstream_inference_prompt_cost\":0.0004,\"upstream_inference_completions_cost\":0.0006},",
    "\"prompt_tokens_details\":{\"cached_tokens\":3},\"completion_tokens_details\":{\"reasoning_tokens\":5}}}\n\n",
    "data: [DONE]\n\n"
);
/// `delta.reasoning` text plus `reasoning_details` fragments (same `index` split across chunks, signature last, one encrypted block).
pub const REASONING: &str = concat!(
    "data: {\"model\":\"anthropic/claude-sonnet-4.5\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"reasoning\":\"Let me \",\"reasoning_details\":[{\"type\":\"reasoning.text\",\"index\":0,\"format\":\"anthropic-claude-v1\",\"text\":\"Let me \"}]},\"finish_reason\":null}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning\":\"think\",\"reasoning_details\":[{\"type\":\"reasoning.text\",\"index\":0,\"text\":\"think\",\"signature\":null}]},\"finish_reason\":null}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_details\":[{\"type\":\"reasoning.text\",\"index\":0,\"text\":\"\",\"signature\":\"sig123\"},{\"type\":\"reasoning.encrypted\",\"index\":1,\"data\":\"abc\"}]},\"finish_reason\":null}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"done\"},\"finish_reason\":null}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}}\n\ndata: [DONE]\n\n"
);
pub const E429: &str = "{\"error\":{\"code\":429,\"message\":\"slow down\",\"metadata\":{\"error_type\":\"rate_limit_exceeded\"}}}";
pub const E402: &str = "{\"error\":{\"code\":402,\"message\":\"Insufficient credits\"}}";
