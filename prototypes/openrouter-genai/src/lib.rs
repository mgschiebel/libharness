//! PROTOTYPE (throwaway): the optional OpenRouter adapter built on the `genai` crate.
//! Exposes the session-task model-call callback shape:
//! `Fn(ModelRequest) -> Pin<Box<dyn Future<Output = Result<ModelReply, String>> + Send>>`.
//! Public genai API only: custom endpoint and auth through its resolvers, `ChatOptions` for
//! capture flags / `extra_body` / the raw frame sink.
mod types;
pub use types::*;

use futures_util::StreamExt;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, ChatStreamEvent, ContentPart, MessageContent, StopReason, Tool, ToolResponse};
use genai::resolver::{AuthData, Endpoint};
use genai::{Client, ServiceTarget};
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

pub type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

#[derive(Clone)]
pub struct Config {
    pub base_url: String,
    pub api_key: String,
    /// OpenRouter model id, e.g. `openai/gpt-4o` (the adapter adds genai's `open_router::` namespace).
    pub model: String,
}

pub struct OpenRouterGenai {
    client: Client,
    model: String,
    /// `Some` = the adapter owns a tokio runtime and runs genai on it (host needs no runtime).
    rt: Option<tokio::runtime::Runtime>,
}

impl OpenRouterGenai {
    pub fn new(cfg: Config) -> Self {
        let (key, base) = (cfg.api_key.clone(), cfg.base_url.clone());
        let client = Client::builder()
            .with_auth_resolver_fn(move |_m| Ok(Some(AuthData::from_single(key.clone()))))
            .with_service_target_resolver_fn(move |mut t: ServiceTarget| {
                t.endpoint = Endpoint::from_owned(base.clone());
                Ok(t)
            })
            .build()
            .expect("genai client");
        Self { client, model: format!("open_router::{}", cfg.model), rt: None }
    }
    /// Workaround for hosts with no tokio: the adapter owns a runtime and spawns each call on it.
    pub fn with_owned_runtime(mut self) -> Self {
        self.rt = Some(tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().expect("tokio runtime"));
        self
    }

    pub fn complete(self: &Arc<Self>, req: ModelRequest) -> BoxFut<Result<ModelReply, ModelError>> {
        let (client, model) = (self.client.clone(), self.model.clone());
        match &self.rt {
            None => Box::pin(run(client, model, req)),
            Some(rt) => {
                // The JoinHandle is waker-based, so any executor can poll it; dropping it must abort.
                let h = rt.spawn(run(client, model, req));
                Box::pin(AbortOnDrop(h))
            }
        }
    }
}

impl Drop for OpenRouterGenai {
    fn drop(&mut self) {
        // A tokio runtime panics if dropped inside async context; the host may drop us anywhere.
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

struct AbortOnDrop(tokio::task::JoinHandle<Result<ModelReply, ModelError>>);
impl Future for AbortOnDrop {
    type Output = Result<ModelReply, ModelError>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx).map(|r| r.unwrap_or_else(|e| Err(ModelError::new(ErrorKind::Transport, format!("task failed: {e}")))))
    }
}
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The callback the host registers. Structured errors are flattened to `String` (the callback's error type).
pub fn model_callback(a: Arc<OpenRouterGenai>) -> impl Fn(ModelRequest) -> BoxFut<Result<ModelReply, String>> + Send + Sync + 'static {
    move |req| {
        let f = a.complete(req);
        Box::pin(async move { f.await.map_err(|e| e.to_string()) })
    }
}

// ---- side channel: what genai's typed events drop, read from raw frames --------------------
#[derive(Default)]
struct Side {
    model: Option<String>,
    cost: Option<f64>,
    cost_details: Option<Value>,
    is_byok: Option<bool>,
    finish_reason: Option<String>,
    reasoning: String,
    reasoning_details: Vec<Value>,
}
fn observe(side: &Mutex<Side>, v: &Value) {
    let mut s = side.lock().unwrap();
    if let Some(m) = v["model"].as_str() {
        s.model = Some(m.to_string());
    }
    if let Some(c) = v["usage"]["cost"].as_f64() {
        s.cost = Some(c);
    }
    if !v["usage"]["cost_details"].is_null() {
        s.cost_details = Some(v["usage"]["cost_details"].clone());
    }
    if let Some(b) = v["usage"]["is_byok"].as_bool() {
        s.is_byok = Some(b);
    }
    if let Some(f) = v["choices"][0]["finish_reason"].as_str() {
        s.finish_reason = Some(f.to_string());
    }
    let d = &v["choices"][0]["delta"];
    if let Some(r) = d["reasoning"].as_str() {
        s.reasoning.push_str(r);
    }
    if let Some(a) = d["reasoning_details"].as_array() {
        s.reasoning_details.extend(a.iter().cloned());
    }
}

// ---- request mapping ------------------------------------------------------------------------
fn to_genai(req: &ModelRequest) -> ChatRequest {
    let mut msgs = Vec::new();
    for m in &req.messages {
        msgs.push(match m {
            Message::User(t) => ChatMessage::user(t.clone()),
            Message::Assistant { text, tool_calls } => {
                let mut parts = Vec::new();
                if !text.is_empty() {
                    parts.push(ContentPart::Text(text.clone()));
                }
                for c in tool_calls {
                    parts.push(ContentPart::ToolCall(genai::chat::ToolCall { call_id: c.id.clone(), fn_name: c.name.clone(), fn_arguments: c.args.clone(), thought_signatures: None }));
                }
                ChatMessage::assistant(MessageContent::from_parts(parts))
            }
            // `is_error` has no slot in genai's ToolResponse (nor in OpenRouter's tool message).
            Message::ToolResult { call_id, name, content, .. } => ChatMessage::from(ToolResponse::new(call_id.clone(), content.clone()).with_fn_name(name.clone())),
        });
    }
    let tools: Vec<Tool> = req.tools.iter().map(|t| Tool::new(t.name.clone()).with_description(t.description.clone()).with_schema(t.params.clone())).collect();
    let mut r = ChatRequest::new(msgs);
    if !tools.is_empty() {
        r = r.with_tools(tools);
    }
    r
}

// ---- the call -------------------------------------------------------------------------------
async fn run(client: Client, model: String, req: ModelRequest) -> Result<ModelReply, ModelError> {
    let side = Arc::new(Mutex::new(Side::default()));
    let sink = side.clone();
    let mut opts = ChatOptions::default()
        .with_capture_usage(true)
        .with_capture_content(true)
        .with_capture_tool_calls(true)
        .with_raw_frame_fn(move |_ctx, f| {
            if let Some(v) = f.json() {
                observe(&sink, &v);
            }
        });
    if let Some(x) = &req.extra {
        opts = opts.with_extra_body(x.clone());
    }
    let partial = |text: &str, side: &Mutex<Side>| Box::new(reply(text.to_string(), vec![], FinishReason::Error, None, side));

    let resp = client.exec_chat_stream(model.as_str(), to_genai(&req), Some(&opts)).await.map_err(|e| map_err(e, None))?;
    let mut stream = resp.stream;
    let mut text = String::new();
    let mut end = None;
    while let Some(ev) = stream.next().await {
        match ev {
            Ok(ChatStreamEvent::Chunk(c)) => text.push_str(&c.content),
            Ok(ChatStreamEvent::End(e)) => end = Some(e),
            Ok(_) => {}
            // genai keeps yielding after an error item, so stop at the first one (dropping the stream closes the socket).
            Err(e) => return Err(map_err(e, Some(partial(&text, &side)))),
        }
    }
    let Some(end) = end else {
        let mut e = ModelError::new(ErrorKind::Truncated, "stream ended without an End event");
        e.partial = Some(partial(&text, &side));
        return Err(e);
    };
    let tool_calls = end.captured_tool_calls().unwrap_or_default().into_iter().map(|c| {
        let (args, args_error) = match &c.fn_arguments {
            Value::String(s) if s.trim().is_empty() => (json!({}), None),
            Value::String(s) => match serde_json::from_str::<Value>(s) {
                Ok(v) => (v, None),
                Err(e) => (Value::String(s.clone()), Some(e.to_string())),
            },
            v => (v.clone(), None),
        };
        ToolCall { id: c.call_id.clone(), name: c.fn_name.clone(), args, args_error }
    });
    let finish = match &end.captured_stop_reason {
        Some(StopReason::Completed(_)) => FinishReason::Stop,
        Some(StopReason::ToolCall(_)) => FinishReason::ToolCalls,
        Some(StopReason::MaxTokens(_)) => FinishReason::Length,
        Some(StopReason::ContentFilter(_)) => FinishReason::ContentFilter,
        Some(StopReason::StopSequence(_)) => FinishReason::Stop,
        Some(StopReason::Other(s)) if s == "error" => FinishReason::Error,
        Some(StopReason::Other(s)) => FinishReason::Other(s.clone()),
        None => FinishReason::Other("none".into()),
    };
    let u = end.captured_usage.as_ref();
    let usage = u.map(|u| Usage { prompt_tokens: u.prompt_tokens.unwrap_or(0) as u64, completion_tokens: u.completion_tokens.unwrap_or(0) as u64, total_tokens: u.total_tokens.unwrap_or(0) as u64, cost: None });
    let r = reply(end.captured_first_text().map(str::to_string).unwrap_or(text), tool_calls.collect(), finish, usage, &side);
    if r.finish == FinishReason::Error {
        let mut e = ModelError::new(ErrorKind::Upstream, "finish_reason was `error` with no error object");
        e.partial = Some(Box::new(r));
        return Err(e);
    }
    Ok(r)
}

fn reply(text: String, tool_calls: Vec<ToolCall>, finish: FinishReason, usage: Option<Usage>, side: &Mutex<Side>) -> ModelReply {
    let s = side.lock().unwrap();
    let usage = match (usage, s.cost) {
        (Some(mut u), c) => {
            u.cost = c;
            Some(u)
        }
        (None, Some(c)) => Some(Usage { cost: Some(c), ..Default::default() }),
        (None, None) => None,
    };
    let mut meta = serde_json::Map::new();
    if let Some(v) = &s.cost_details {
        meta.insert("cost_details".into(), v.clone());
    }
    if let Some(v) = s.is_byok {
        meta.insert("is_byok".into(), v.into());
    }
    if !s.reasoning.is_empty() {
        meta.insert("reasoning".into(), s.reasoning.clone().into());
    }
    if !s.reasoning_details.is_empty() {
        meta.insert("reasoning_details".into(), Value::Array(s.reasoning_details.clone()));
    }
    ModelReply { text, tool_calls, finish, usage, model: s.model.clone(), meta: (!meta.is_empty()).then(|| Value::Object(meta)) }
}

// ---- error flattening -----------------------------------------------------------------------
fn kind_from_code(code: i64) -> ErrorKind {
    match code {
        400 | 413 | 422 => ErrorKind::BadRequest,
        401 | 403 => ErrorKind::Auth,
        402 => ErrorKind::Credits,
        429 => ErrorKind::RateLimited,
        _ => ErrorKind::Upstream,
    }
}
fn http_err(status: u16, body: &str, retry_after: Option<u64>) -> ModelError {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let msg = v["error"]["message"].as_str().map(str::to_string).unwrap_or_else(|| body.chars().take(200).collect());
    let mut e = ModelError::new(kind_from_code(status as i64), msg);
    e.status = Some(status);
    e.retry_after_secs = retry_after;
    e.metadata = v["error"].get("metadata").cloned();
    e
}
fn map_err(e: genai::Error, partial: Option<Box<ModelReply>>) -> ModelError {
    use genai::Error as G;
    let mut out = match e {
        G::ChatResponse { body, .. } => {
            let code = body["code"].as_i64();
            let mut m = ModelError::new(code.map(kind_from_code).unwrap_or(ErrorKind::Upstream), body["message"].as_str().unwrap_or("error chunk").to_string());
            m.code = code;
            m.metadata = body.get("metadata").cloned();
            m
        }
        G::HttpError { status, body, headers, .. } => http_err(status.as_u16(), &body, headers.get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse().ok())),
        G::WebStream { error, cause, .. } => match error.downcast_ref::<G>() {
            Some(G::HttpError { status, body, headers, .. }) => http_err(status.as_u16(), body, headers.get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse().ok())),
            _ => ModelError::new(ErrorKind::Transport, cause),
        },
        G::StreamParse { serde_error, .. } => ModelError::new(ErrorKind::Protocol, serde_error.to_string()),
        other => ModelError::new(ErrorKind::Transport, other.to_string()),
    };
    out.partial = partial;
    out
}
