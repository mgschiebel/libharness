//! The adapter glue: library request -> openrouter-rs request, unified stream -> library reply.
use crate::model::*;
use futures_util::{FutureExt, StreamExt};
use openrouter_rs::api::chat::{ChatCompletionRequest, Content, Message as OrMessage, Plugin};
use openrouter_rs::error::OpenRouterError;
use openrouter_rs::types::provider::ProviderPreferences;
use openrouter_rs::types::stream::UnifiedStreamEvent;
use openrouter_rs::types::{ReasoningConfig, Role, Tool, ToolCall as OrToolCall};
use openrouter_rs::OpenRouterClient;
use serde_json::Value;
use std::collections::BTreeSet;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;

/// Decode `extra[key]` into the crate's typed request field (its `deny_unknown_fields` rejects typos).
macro_rules! take {
    ($extra:expr, $key:literal, $ty:ty) => {
        match $extra.get($key) {
            None => None,
            Some(v) => {
                Some(serde_json::from_value::<$ty>(v.clone()).map_err(|e| {
                    ModelError::new(ErrorKind::Config, format!("extra.{}: {e}", $key))
                })?)
            }
        }
    };
}

pub type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

pub struct OpenRouterAdapter {
    client: OpenRouterClient,
    model: String,
}

impl OpenRouterAdapter {
    /// `base_url` is the crate's base-URL setting (the real one is `https://openrouter.ai/api/v1`).
    pub fn new(base_url: &str, api_key: &str, model: &str) -> Result<Self, ModelError> {
        let client = OpenRouterClient::builder()
            .base_url(base_url)
            .api_key(api_key)
            .build()
            .map_err(flatten)?;
        Ok(Self {
            client,
            model: model.to_string(),
        })
    }

    /// One model call. `sink` sees each text delta as it arrives.
    pub async fn call(
        &self,
        req: &ModelRequest,
        sink: &(dyn Fn(&str) + Send + Sync),
    ) -> Result<ModelReply, ModelError> {
        let request = self.encode(req)?;
        let mut stream = self
            .client
            .chat()
            .stream_unified(&request)
            .await
            .map_err(flatten)?;
        let mut text = String::new();
        let mut tool_indexes = BTreeSet::new();
        loop {
            // the crate's SSE parser panics when the body ends inside an event, so poll under catch_unwind
            let event = match AssertUnwindSafe(stream.next()).catch_unwind().await {
                Ok(Some(event)) => event,
                Ok(None) => break,
                Err(_) => {
                    return Err(with_partial(
                        ModelError::new(
                            ErrorKind::Protocol,
                            "openrouter-rs stream panicked (body ended inside an SSE event)",
                        ),
                        partial(&text),
                    ))
                }
            };
            match event {
                UnifiedStreamEvent::ContentDelta(t) => {
                    sink(&t);
                    text.push_str(&t);
                }
                UnifiedStreamEvent::ToolDelta(v) => {
                    tool_indexes.insert(v["index"].as_u64().unwrap_or(0));
                }
                UnifiedStreamEvent::Error(e) => {
                    // first error ends the call; dropping `stream` closes the connection
                    let mut err = flatten(e);
                    err.partial = Some(Box::new(partial(&text)));
                    return Err(err);
                }
                UnifiedStreamEvent::Done {
                    model,
                    finish_reason,
                    tool_calls,
                    usage,
                    ..
                } => {
                    let mut reply = ModelReply {
                        text,
                        tool_calls: vec![],
                        finish: finish(finish_reason.as_deref()),
                        usage: None,
                        model,
                    };
                    reply.usage = usage.map(|u| Usage {
                        prompt_tokens: u.prompt_tokens.into(),
                        completion_tokens: u.completion_tokens.into(),
                        total_tokens: u.total_tokens.into(),
                        cost: u.cost,
                        is_byok: u.is_byok,
                        cost_details: u
                            .cost_details
                            .as_ref()
                            .and_then(|c| serde_json::to_value(c).ok()),
                    });
                    reply.tool_calls = tool_calls.iter().map(convert_call).collect();
                    return match finish_reason.as_deref() {
                        Some("error") => Err(with_partial(ModelError::new(ErrorKind::Upstream, "stream finished with finish_reason=error (error object not exposed by the crate)"), reply)),
                        None => Err(with_partial(ModelError::new(ErrorKind::Truncated, "stream ended without a finish reason"), reply)),
                        // the crate silently drops a tool call that never had an id or name; do not pass that on
                        _ if reply.tool_calls.len() < tool_indexes.len() => Err(with_partial(ModelError::new(ErrorKind::Protocol, "a streamed tool call lacked an id or a name and was dropped by the crate"), reply)),
                        _ => Ok(reply),
                    };
                }
                _ => {} // reasoning deltas, raw events: dropped (lossy)
            }
        }
        Err(ModelError::new(
            ErrorKind::Truncated,
            "unified stream ended without a Done event",
        ))
    }

    fn encode(&self, req: &ModelRequest) -> Result<ChatCompletionRequest, ModelError> {
        let mut b = ChatCompletionRequest::builder();
        let messages = req.messages.iter().map(convert_message).collect::<Vec<_>>();
        b.model(self.model.as_str()).messages(messages);
        for t in &req.tools {
            b.tool(Tool::new(&t.name, &t.description, t.params.clone()));
        }
        if let Some(extra) = &req.extra {
            if let Some(models) = take!(extra, "models", Vec<String>) {
                b.models(models);
            }
            if let Some(p) = take!(extra, "provider", ProviderPreferences) {
                b.provider(p);
            }
            if let Some(r) = take!(extra, "reasoning", ReasoningConfig) {
                b.reasoning(r);
            }
            if let Some(p) = take!(extra, "plugins", Vec<Plugin>) {
                b.plugins(p);
            }
        }
        b.build().map_err(flatten)
    }
}

fn convert_message(m: &Message) -> OrMessage {
    match m {
        Message::User(t) => OrMessage::new(Role::User, t.as_str()),
        Message::Assistant { text, tool_calls } => {
            let content = if text.is_empty() {
                Content::Null
            } else {
                Content::Text(text.clone())
            };
            let calls = tool_calls
                .iter()
                .map(|c| OrToolCall::new(&c.id, &c.name, c.args.to_string()))
                .collect();
            OrMessage::assistant_with_tool_calls(content, calls)
        }
        // `is_error` has no slot on a tool message.
        Message::ToolResult {
            call_id, content, ..
        } => OrMessage::tool_response(call_id, content.as_str()),
    }
}

fn convert_call(c: &OrToolCall) -> ToolCall {
    let (args, args_error) = if c.function.arguments.trim().is_empty() {
        (Value::Object(Default::default()), None)
    } else {
        match serde_json::from_str(&c.function.arguments) {
            Ok(v) => (v, None),
            Err(e) => (Value::Null, Some(e.to_string())),
        }
    };
    ToolCall {
        id: c.id.clone(),
        name: c.function.name.clone(),
        args,
        args_error,
    }
}

fn finish(reason: Option<&str>) -> FinishReason {
    match reason {
        Some("stop") => FinishReason::Stop,
        Some("tool_calls") => FinishReason::ToolCalls,
        Some("length") => FinishReason::Length,
        Some("content_filter") => FinishReason::ContentFilter,
        other => FinishReason::Other(other.unwrap_or("").to_string()),
    }
}

fn partial(text: &str) -> ModelReply {
    ModelReply {
        text: text.to_string(),
        tool_calls: vec![],
        finish: FinishReason::Other("error".into()),
        usage: None,
        model: None,
    }
}
fn with_partial(mut e: ModelError, reply: ModelReply) -> ModelError {
    e.partial = Some(Box::new(reply));
    e
}

/// Crate error -> library error. `OpenRouterError` is `#[non_exhaustive]`, so the wildcard is forced.
pub fn flatten(e: OpenRouterError) -> ModelError {
    match e {
        OpenRouterError::Api(c) => {
            let kind = match c.status.as_u16() {
                400 | 413 | 422 => ErrorKind::BadRequest,
                401 | 403 => ErrorKind::Auth,
                402 => ErrorKind::Credits,
                429 => ErrorKind::RateLimited,
                _ => ErrorKind::Upstream,
            };
            ModelError {
                kind,
                status: Some(c.status.as_u16()),
                code: c.api_code,
                message: c.message.clone(),
                retry_after_secs: c.retry_after.map(|d| d.as_secs()),
                metadata: c.metadata.clone(),
                partial: None,
            }
        }
        OpenRouterError::HttpRequest(h) => ModelError::new(ErrorKind::Transport, h.to_string()),
        OpenRouterError::Io(e) => ModelError::new(ErrorKind::Transport, e.to_string()),
        OpenRouterError::Serialization(e) => ModelError::new(ErrorKind::Protocol, e.to_string()),
        other => ModelError::new(ErrorKind::Config, other.to_string()),
    }
}

/// The session-task model-call callback shape. The structured error is flattened to `String`.
pub fn model_callback(
    adapter: Arc<OpenRouterAdapter>,
) -> impl Fn(ModelRequest) -> BoxFut<Result<ModelReply, String>> + Send + Sync + 'static {
    move |req| {
        let a = adapter.clone();
        Box::pin(async move { a.call(&req, &|_| {}).await.map_err(|e| e.to_string()) })
    }
}
