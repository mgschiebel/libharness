//! `reqwest` 0.13 with `stream`. The futures are native async and need a tokio runtime entered by the poller.
use futures_util::stream::{BoxStream, StreamExt};
use provider_adapter::provider_codec::{HttpRequest, ResponseHead};
use provider_adapter::{Body, BoxFut, Transport};

#[derive(Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
    #[cfg(feature = "owned-runtime")]
    rt: Option<std::sync::Arc<OwnedRt>>,
}
impl ReqwestTransport {
    /// The futures need a tokio runtime entered by whoever polls them.
    pub fn new() -> Self {
        Self { client: reqwest::Client::builder().no_proxy().build().expect("client"), #[cfg(feature = "owned-runtime")] rt: None }
    }
    /// Workaround: the adapter owns a one-thread tokio runtime and the host polls a runtime-neutral bridge future.
    #[cfg(feature = "owned-runtime")]
    pub fn with_owned_runtime() -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).thread_name("lh-reqwest-rt").enable_all().build().expect("runtime");
        Self { rt: Some(std::sync::Arc::new(OwnedRt(Some(rt)))), ..Self::new() }
    }
}
/// Dropping a `Runtime` on a thread that is inside another runtime panics ("Cannot drop a runtime in a
/// context where blocking is not allowed"), and the host drops its transport wherever it likes.
#[cfg(feature = "owned-runtime")]
struct OwnedRt(Option<tokio::runtime::Runtime>);
#[cfg(feature = "owned-runtime")]
impl Drop for OwnedRt {
    fn drop(&mut self) {
        self.0.take().map(|rt| rt.shutdown_background());
    }
}
struct ReqwestBody(BoxStream<'static, Result<Vec<u8>, String>>);
impl Body for ReqwestBody {
    fn next_chunk(&mut self) -> BoxFut<'_, Option<Result<Vec<u8>, String>>> {
        Box::pin(async move { self.0.next().await })
    }
}
impl Transport for ReqwestTransport {
    fn send(&self, req: HttpRequest) -> BoxFut<'static, Result<(ResponseHead, Box<dyn Body>), String>> {
        let client = self.client.clone();
        #[cfg(feature = "owned-runtime")]
        if let Some(rt) = self.rt.clone() {
            return Box::pin(async move {
                let (tx, rx) = provider_adapter::bridge::channel();
                let task = rt.0.as_ref().unwrap().spawn(async move {
                    use provider_adapter::bridge::Item;
                    match start(&client, req).await {
                        Err(e) => drop(tx.send_nowait(Item::Fail(e))),
                        Ok((head, mut body)) => {
                            if !tx.send_nowait(Item::Head(head)) {
                                return;
                            }
                            while let Some(c) = body.0.next().await {
                                let item = c.map(Item::Chunk).unwrap_or_else(Item::Fail);
                                if !tx.send_nowait(item) {
                                    return;
                                }
                            }
                        }
                    }
                });
                rx.on_cancel(move || task.abort()); // aborting the task drops the response: the connection closes
                provider_adapter::bridge::attach(rx).await
            });
        }
        Box::pin(async move { start(&client, req).await.map(|(h, b)| (h, Box::new(b) as Box<dyn Body>)) })
    }
}

async fn start(client: &reqwest::Client, req: HttpRequest) -> Result<(ResponseHead, ReqwestBody), String> {
    let mut b = client.post(&req.url).body(req.body);
    for (k, v) in &req.headers {
        b = b.header(k.as_str(), v.as_str());
    }
    let resp = b.send().await.map_err(|e| e.to_string())?;
    let headers = resp.headers().iter().map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect();
    let head = ResponseHead { status: resp.status().as_u16(), headers };
    Ok((head, ReqwestBody(resp.bytes_stream().map(|r| r.map(|b| b.to_vec()).map_err(|e| e.to_string())).boxed())))
}
