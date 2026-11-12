//! Optional workaround for the tokio requirement: the adapter owns a tokio runtime, runs the whole
//! call on it, and hands the callback a future that any executor can poll.
use crate::adapter::{BoxFut, OpenRouterAdapter};
use crate::model::*;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::runtime::Runtime;
use tokio::task::{JoinError, JoinHandle};

/// A `JoinHandle` does not cancel its task when dropped; this one does (that is the abort path).
struct AbortOnDrop<T>(JoinHandle<T>);
impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}
impl<T> Future for AbortOnDrop<T> {
    type Output = Result<T, JoinError>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

pub fn model_callback_private_runtime(
    adapter: Arc<OpenRouterAdapter>,
    rt: Arc<Runtime>,
) -> impl Fn(ModelRequest) -> BoxFut<Result<ModelReply, String>> + Send + Sync + 'static {
    move |req| {
        let a = adapter.clone();
        let task = AbortOnDrop(
            rt.spawn(async move { a.call(&req, &|_| {}).await.map_err(|e| e.to_string()) }),
        );
        Box::pin(async move { task.await.map_err(|e| format!("task: {e}"))? })
    }
}
