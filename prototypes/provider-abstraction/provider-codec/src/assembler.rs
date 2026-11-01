//! Provider-independent: folds `StreamEvent`s into one `ModelReply`.
use crate::types::*;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use serde_json::Value;

#[derive(Default)]
struct Partial {
    id: Option<String>,
    name: String,
    args: String,
}
#[derive(Default)]
struct PartialReasoning {
    text: String,
    signature: Option<String>,
    redacted: Option<String>,
}
#[derive(Default)]
pub struct Assembler {
    text: String,
    reasoning: BTreeMap<usize, PartialReasoning>,
    calls: BTreeMap<usize, Partial>,
    finish: Option<FinishReason>,
    usage: Option<Usage>,
    model: Option<String>,
    error: Option<ProviderError>,
}
impl Assembler {
    pub fn push(&mut self, ev: StreamEvent) {
        if self.error.is_some() {
            return; // an error ends the stream; ignore anything after it
        }
        match ev {
            StreamEvent::Text(t) => self.text.push_str(&t),
            StreamEvent::ToolCall { index, id, name, args } => {
                let p = self.calls.entry(index).or_default();
                if id.is_some() && p.id.is_none() {
                    p.id = id;
                }
                if let Some(n) = name {
                    if p.name.is_empty() {
                        p.name = n;
                    }
                }
                p.args.push_str(&args);
            }
            StreamEvent::Reasoning { index, text, signature } => {
                let r = self.reasoning.entry(index).or_default();
                r.text.push_str(&text);
                if signature.is_some() {
                    r.signature = signature;
                }
            }
            StreamEvent::RedactedReasoning { index, data } => {
                self.reasoning.entry(index).or_default().redacted = Some(data);
            }
            // OpenRouter repeats the terminal reason in the usage chunk: first one wins.
            StreamEvent::Finish(f) => {
                self.finish.get_or_insert(f);
            }
            StreamEvent::Usage(u) => self.usage = Some(u),
            StreamEvent::Model(m) => self.model = Some(m),
            StreamEvent::Error(e) => self.error = Some(e),
        }
    }
    pub fn has_error(&self) -> bool {
        self.error.is_some()
    }
    fn reply(&mut self, finish: FinishReason) -> ModelReply {
        let calls = core::mem::take(&mut self.calls)
            .into_iter()
            .map(|(i, p)| {
                let (args, args_error) = if p.args.trim().is_empty() {
                    (Value::Object(Default::default()), None) // no-arg tools often send ""
                } else {
                    match serde_json::from_str(&p.args) {
                        Ok(v) => (v, None),
                        Err(e) => (Value::Null, Some(format!("invalid JSON arguments: {e}"))),
                    }
                };
                ToolCall { id: p.id.unwrap_or_else(|| format!("call_{i}")), name: p.name, args, args_error }
            })
            .collect::<Vec<_>>();
        let reasoning = core::mem::take(&mut self.reasoning)
            .into_values()
            .map(|r| match r.redacted {
                Some(data) => ReasoningBlock::Redacted { data },
                None => ReasoningBlock::Thinking { text: r.text, signature: r.signature.unwrap_or_default() },
            })
            .collect();
        ModelReply {
            text: core::mem::take(&mut self.text),
            tool_calls: calls,
            reasoning,
            finish,
            usage: self.usage.take(),
            model: self.model.take(),
        }
    }
    pub fn finish(mut self) -> Result<ModelReply, ProviderError> {
        if let Some(mut e) = self.error.take() {
            e.partial = Some(Box::new(self.reply(FinishReason::Error)));
            return Err(e);
        }
        match self.finish.take() {
            None => {
                let mut e = ProviderError::new(ErrorKind::Truncated, "stream ended without a finish reason");
                e.partial = Some(Box::new(self.reply(FinishReason::Error)));
                Err(e)
            }
            Some(FinishReason::Error) => {
                let mut e = ProviderError::new(ErrorKind::Upstream, "finish_reason=error with no error object");
                e.partial = Some(Box::new(self.reply(FinishReason::Error)));
                Err(e)
            }
            Some(f) => Ok(self.reply(f)),
        }
    }
}
