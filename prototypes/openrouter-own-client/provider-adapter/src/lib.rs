//! PROTOTYPE (throwaway). The std-side half: the only place that awaits. It owns the `Transport`
//! seam (host supplies the HTTP client), drives any `Provider`, and exposes the result as the
//! session loop's model-call callback shape (`Fn(ModelRequest) -> Future<Result<ModelReply, String>>`).
//! No runtime types: only `std::future` + boxed futures.
use provider_codec::*;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

pub use provider_codec;

pub mod bridge;

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Response body as pull-based chunks. Dropping it must close the connection (that is the cancel path).
pub trait Body: Send {
    /// The future may borrow the body (prototype #7 had `'static` here, which forced `Arc<Mutex<stream>>`
    /// in any body that owns a stream: changed to `'_`, see README).
    fn next_chunk(&mut self) -> BoxFut<'_, Option<Result<Vec<u8>, String>>>;
}
/// The host's HTTP client, hidden behind one method.
pub trait Transport: Send + Sync {
    fn send(&self, req: HttpRequest) -> BoxFut<'static, Result<(ResponseHead, Box<dyn Body>), String>>;
}

/// One model call. `sink` sees every normalized event as it arrives (live deltas); optional.
pub async fn complete<P, T>(p: &P, t: &T, req: &ModelRequest, sink: &(dyn Fn(&StreamEvent) + Send + Sync)) -> Result<ModelReply, ProviderError>
where
    P: Provider,
    T: Transport + ?Sized,
{
    let (head, mut body) = t.send(p.request(req)).await.map_err(|e| ProviderError::new(ErrorKind::Transport, e))?;
    let mut dec = p.decoder(&head);
    let mut asm = Assembler::default();
    let mut evs = Vec::new();
    let mut transport_err = None;
    loop {
        match body.next_chunk().await {
            Some(Ok(bytes)) => dec.push(&bytes, &mut evs),
            Some(Err(e)) => {
                transport_err = Some(e);
                break;
            }
            None => break,
        }
        drain(&mut evs, &mut asm, sink);
        if asm.has_error() {
            return asm.finish(); // stop reading; dropping `body` closes the connection
        }
    }
    if let Some(e) = transport_err {
        asm.push(StreamEvent::Error(ProviderError::new(ErrorKind::Transport, e)));
    } else {
        dec.finish(&mut evs);
    }
    drain(&mut evs, &mut asm, sink);
    asm.finish()
}
fn drain(evs: &mut Vec<StreamEvent>, asm: &mut Assembler, sink: &(dyn Fn(&StreamEvent) + Send + Sync)) {
    for e in evs.drain(..) {
        sink(&e);
        asm.push(e);
    }
}

/// The optional adapter: register the result as the host's model capability. Structured errors
/// are flattened to `String` because that is the callback's error type (see README: leak).
pub fn model_callback<P, T>(p: Arc<P>, t: Arc<T>) -> impl Fn(ModelRequest) -> BoxFut<'static, Result<ModelReply, String>> + Send + Sync + 'static
where
    P: Provider + Send + Sync + 'static,
    P::Decoder: Send,
    T: Transport + ?Sized + 'static,
{
    move |req| {
        let (p, t) = (p.clone(), t.clone());
        Box::pin(async move { complete(&*p, &*t, &req, &|_| {}).await.map_err(|e| e.to_string()) })
    }
}

/// Host-supplied timer. The library has no timers (ADR 0002), so waiting before a retry is the host's.
pub type Sleep = dyn Fn(Duration) -> BoxFut<'static, ()> + Send + Sync;

/// Retry policy glue: only when the error is retryable AND nothing was delivered yet (a retry after
/// partial text would duplicate it). Delay is `Retry-After` when present, else 1s, 2s, 4s...; capped.
/// No jitter: the library has no randomness either.
pub async fn complete_with_retry<P, T>(p: &P, t: &T, req: &ModelRequest, sleep: &Sleep, max_attempts: u32, cap: Duration) -> Result<ModelReply, ProviderError>
where
    P: Provider,
    T: Transport + ?Sized,
{
    let mut attempt = 0;
    loop {
        attempt += 1;
        match complete(p, t, req, &|_| {}).await {
            Err(e) if attempt < max_attempts && e.retryable() && e.partial.as_ref().map_or(true, |r| r.text.is_empty() && r.tool_calls.is_empty()) => {
                let d = e.retry_after_secs.map(Duration::from_secs).unwrap_or(Duration::from_secs(1 << (attempt - 1))).min(cap);
                sleep(d).await;
            }
            r => return r,
        }
    }
}
/// `model_callback` with retry; the extra capability is the sleeper.
pub fn model_callback_retrying<P, T>(p: Arc<P>, t: Arc<T>, sleep: Arc<Sleep>, max_attempts: u32, cap: Duration) -> impl Fn(ModelRequest) -> BoxFut<'static, Result<ModelReply, String>> + Send + Sync + 'static
where
    P: Provider + Send + Sync + 'static,
    P::Decoder: Send,
    T: Transport + ?Sized + 'static,
{
    move |req| {
        let (p, t, sleep) = (p.clone(), t.clone(), sleep.clone());
        Box::pin(async move { complete_with_retry(&*p, &*t, &req, &*sleep, max_attempts, cap).await.map_err(|e| e.to_string()) })
    }
}
