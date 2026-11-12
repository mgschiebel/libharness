//! Host-registered callbacks: the model call, every tool, and the typed-result validator.
//! Boxed `Send + Sync + 'static`, as the triage decided. No runtime types.
use crate::*;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::ToString;
use alloc::sync::Arc;
use core::future::Future;
use core::pin::Pin;
use serde::de::DeserializeOwned;

pub type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
pub type ModelFn = Arc<dyn Fn(ModelRequest, CancelToken) -> BoxFut<Result<ModelReply, String>> + Send + Sync>;
pub type ToolFn = Arc<dyn Fn(ToolCtx, Value) -> BoxFut<Result<Value, String>> + Send + Sync>;
pub type ValidateFn = Arc<dyn Fn(&Value) -> Result<(), String> + Send + Sync>;

/// Registered up front, cloned cheaply (all `Arc`s), reusable across any number of runs.
#[derive(Clone)]
pub struct Callbacks {
    pub spec: Spec,
    pub validate: ValidateFn,
    pub(crate) model: Option<ModelFn>,
    pub(crate) tools: alloc::vec::Vec<(String, ToolFn)>,
}

impl Callbacks {
    pub fn new(result_schema: Value, validate: impl Fn(&Value) -> Result<(), String> + Send + Sync + 'static) -> Self {
        Self {
            spec: Spec { tools: alloc::vec![], result_schema },
            validate: Arc::new(validate),
            model: None,
            tools: alloc::vec![],
        }
    }
    /// The host picks the result type `R`; the core only ever sees `Value`.
    pub fn typed<R: DeserializeOwned>(result_schema: Value) -> Self {
        Self::new(result_schema, |v| serde_json::from_value::<R>(v.clone()).map(|_| ()).map_err(|e| e.to_string()))
    }
    /// The model call is just another capability.
    pub fn model<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(ModelRequest, CancelToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<ModelReply, String>> + Send + 'static,
    {
        self.model = Some(Arc::new(move |r, t| Box::pin(f(r, t))));
        self
    }
    /// Untyped tool: JSON in, JSON out.
    pub fn tool<F, Fut>(mut self, name: &str, description: &str, params: Value, f: F) -> Self
    where
        F: Fn(ToolCtx, Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value, String>> + Send + 'static,
    {
        self.spec.tools.push(ToolSpec { name: name.into(), description: description.into(), params });
        self.tools.push((name.into(), Arc::new(move |c, v| Box::pin(f(c, v)))));
        self
    }
    /// Typed tool: the host's closure sees `I` and returns `O`; JSON stays at the edge.
    pub fn tool_typed<I, O, F, Fut>(self, name: &str, description: &str, params: Value, f: F) -> Self
    where
        I: DeserializeOwned + Send + 'static,
        O: serde::Serialize + 'static,
        F: Fn(ToolCtx, I) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<O, String>> + Send + 'static,
    {
        let f = Arc::new(f);
        self.tool(name, description, params, move |ctx, v| {
            let f = f.clone();
            async move {
                let input: I = serde_json::from_value(v).map_err(|e| format!("bad arguments: {e}"))?;
                let out = f(ctx, input).await?;
                serde_json::to_value(out).map_err(|e| e.to_string())
            }
        })
    }
}
