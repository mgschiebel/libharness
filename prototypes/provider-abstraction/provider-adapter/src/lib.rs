//! PROTOTYPE (throwaway). The std-side half: the only place that awaits. It owns the `Transport`
//! seam (host supplies the HTTP client), drives any `Provider`, and exposes the result as the
//! session loop's model-call callback shape (`Fn(ModelRequest) -> Future<Result<ModelReply, String>>`).
//! No runtime types: only `std::future` + boxed futures.
use provider_codec::*;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub use provider_codec;

pub type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Response body as pull-based chunks. Dropping it must close the connection (that is the cancel path).
pub trait Body: Send {
    fn next_chunk(&mut self) -> BoxFut<Option<Result<Vec<u8>, String>>>;
}
/// The host's HTTP client, hidden behind one method.
pub trait Transport: Send + Sync {
    fn send(&self, req: HttpRequest) -> BoxFut<Result<(ResponseHead, Box<dyn Body>), String>>;
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
pub fn model_callback<P, T>(p: Arc<P>, t: Arc<T>) -> impl Fn(ModelRequest) -> BoxFut<Result<ModelReply, String>> + Send + Sync + 'static
where
    P: Provider + Send + Sync + 'static,
    P::Decoder: Send,
    T: Transport + 'static,
{
    move |req| {
        let (p, t) = (p.clone(), t.clone());
        Box::pin(async move { complete(&*p, &*t, &req, &|_| {}).await.map_err(|e| e.to_string()) })
    }
}
