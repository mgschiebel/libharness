//! B2 (and the library half of B3): every argument, reply and event is JSON bytes, and the
//! handles are replaced by plain callbacks.
//!
//! `Wire` is the whole boundary, bytes in and bytes out, and it is the same trait whether the
//! other side is a closure in this process (B2) or a Wasm host's imports seen from the guest
//! (B3). `Link2` is the library's use of it: it encodes, calls, decodes.
//!   handle                   becomes
//!   `CancelToken` (hooks)    `Plain::cancelled()`: poll only, no hook, no waker
//!   event `Sink`             `Plain::emit(bytes)`
//!   response body stream     `call("body_next", handle)` per chunk and `close("body", handle)`
//! Chunks cross as raw bytes, not JSON (JSON has no byte string; base64 would add a third).
use crate::link::*;
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use call_core::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The plain callbacks a host callback may call back into, in place of handles.
#[derive(Clone)]
pub struct Plain {
    pub cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    pub emit: Arc<dyn Fn(&[u8]) + Send + Sync>,
}

/// The byte-level boundary. Operations: "model", "tool", "transport", "body_next".
pub trait Wire: Send + Sync {
    fn call(&self, op: &str, args: Vec<u8>, plain: Plain) -> BoxFut<Result<Vec<u8>, String>>;
    fn validate(&self, v: &[u8]) -> Result<(), String>;
    /// JSON `Vec<(Inject, String)>`
    fn inbox(&self) -> Vec<u8>;
    fn emit(&self, ev: &[u8]);
    /// Release a host-side resource (a response body).
    fn close(&self, kind: &str, handle: u64);
}

#[derive(Serialize, Deserialize)]
pub struct ToolIn {
    pub call_id: String,
    pub name: String,
    pub args: Value,
}
#[derive(Serialize, Deserialize)]
pub struct TransportOut {
    pub head: HttpHead,
    pub body: u64,
}

pub fn enc<T: Serialize>(t: &T) -> Vec<u8> {
    serde_json::to_vec(t).expect("encode")
}
fn dec<T: for<'a> Deserialize<'a>>(b: &[u8]) -> Result<T, String> {
    serde_json::from_slice(b).map_err(|e| e.to_string())
}

pub struct Link2 {
    pub wire: Arc<dyn Wire>,
    pub token: CancelToken,
}
impl Link2 {
    fn plain(&self) -> Plain {
        let (t, w) = (self.token.clone(), self.wire.clone());
        Plain { cancelled: Arc::new(move || t.is_cancelled()), emit: Arc::new(move |b| w.emit(b)) }
    }
}

struct Body2 {
    wire: Arc<dyn Wire>,
    plain: Plain,
    handle: u64,
}
impl BodyStream for Body2 {
    fn next<'a>(&'a mut self) -> BoxFutR<'a, Option<Vec<u8>>> {
        let f = self.wire.call("body_next", self.handle.to_le_bytes().to_vec(), self.plain.clone());
        Box::pin(async move {
            match f.await {
                Ok(b) if b.first() == Some(&1) => Some(b[1..].to_vec()),
                _ => None,
            }
        })
    }
}
impl Drop for Body2 {
    fn drop(&mut self) {
        self.wire.close("body", self.handle);
    }
}

impl Link for Link2 {
    fn model(&self, req: ModelRequest) -> BoxFut<Result<ModelReply, String>> {
        let f = self.wire.call("model", enc(&req), self.plain());
        Box::pin(async move { dec::<ModelReply>(&f.await?) })
    }
    fn tool(&self, call: ToolCall) -> BoxFut<Result<Value, String>> {
        let f = self.wire.call("tool", enc(&ToolIn { call_id: call.id, name: call.name, args: call.args }), self.plain());
        Box::pin(async move { dec::<Result<Value, String>>(&f.await?)? })
    }
    fn transport(&self, req: HttpReq) -> BoxFut<Result<(HttpHead, Box<dyn BodyStream>), String>> {
        let (wire, plain) = (self.wire.clone(), self.plain());
        let f = wire.call("transport", enc(&req), plain.clone());
        Box::pin(async move {
            let o: TransportOut = dec(&f.await?)?;
            Ok((o.head, Box::new(Body2 { wire, plain, handle: o.body }) as Box<dyn BodyStream>))
        })
    }
    fn validate(&self, v: &Value) -> Result<(), String> {
        self.wire.validate(&enc(v))
    }
    fn inbox(&self) -> Vec<(Inject, String)> {
        let b = self.wire.inbox();
        if b.is_empty() { Vec::new() } else { dec(&b).unwrap_or_default() }
    }
    fn emit(&self, ev: &Ev) {
        self.wire.emit(&enc(ev))
    }
    fn cancel(&self) -> &CancelToken {
        &self.token
    }
}
