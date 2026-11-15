//! The host's side of the byte-level boundary (`Wire`), used by B2 in-process and by B3 behind
//! the wasmtime imports, plus the meter that counts what crosses.
use crate::host::*;
use cb_core::b2::*;
use cb_core::*;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Mutex};

/// Crossings and bytes, by name. A crossing is one call over the boundary in either direction.
#[derive(Default)]
pub struct Meter {
    by: Mutex<BTreeMap<&'static str, (u64, u64)>>,
}
impl Meter {
    pub fn cross(&self, name: &'static str, bytes: usize) {
        let mut g = self.by.lock().unwrap();
        let e = g.entry(name).or_default();
        e.0 += 1;
        e.1 += bytes as u64;
    }
    pub fn reset(&self) {
        self.by.lock().unwrap().clear();
    }
    pub fn snapshot(&self) -> BTreeMap<&'static str, (u64, u64)> {
        self.by.lock().unwrap().clone()
    }
    pub fn total(&self) -> (u64, u64) {
        self.by.lock().unwrap().values().fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1))
    }
    /// Crossings that carry a payload (not the bookkeeping calls: poll, wait, cancelled).
    pub fn payload(&self) -> (u64, u64) {
        self.by.lock().unwrap().iter().filter(|(k, _)| !matches!(**k, "op_poll" | "wait" | "cancelled")).fold((0, 0), |a, (_, b)| (a.0 + b.0, a.1 + b.1))
    }
}

pub type Observer = Arc<dyn Fn(Ev) + Send + Sync>;

/// The host's wire: decodes the bytes, calls the host's natural callbacks, encodes the reply.
pub struct HostWire {
    pub hc: HostCaps,
    pub observer: Observer,
    pub inbox: Inbox,
    /// Optional: the host's OWN token for this run, shown to callbacks (never crosses the wire).
    pub host_token: Option<CancelToken>,
    bodies: Arc<Mutex<HashMap<u64, Option<Box<dyn BodyStream>>>>>,
    next_body: AtomicU64,
}
impl HostWire {
    pub fn new(hc: HostCaps, observer: Observer, inbox: Inbox) -> Self {
        HostWire { hc, observer, inbox, host_token: None, bodies: Default::default(), next_body: AtomicU64::new(1) }
    }
    pub fn open_bodies(&self) -> usize {
        self.bodies.lock().unwrap().len()
    }
}
fn de<T: for<'a> serde::Deserialize<'a>>(b: &[u8]) -> Result<T, String> {
    serde_json::from_slice(b).map_err(|e| e.to_string())
}
impl Wire for HostWire {
    fn call(&self, op: &str, args: Vec<u8>, plain: Plain) -> BoxFut<Result<Vec<u8>, String>> {
        let p2 = plain.clone();
        let mut api = Api::from_plain(plain.cancelled.clone(), Arc::new(move |e| (p2.emit)(&enc(&e))));
        api.token = self.host_token.clone();
        let hc = self.hc.clone();
        match op {
            "model" => {
                let req = match de::<ModelRequest>(&args) { Ok(r) => r, Err(e) => return Box::pin(async move { Err(e) }) };
                let f = (hc.model)(req, api);
                Box::pin(async move { Ok(enc(&f.await?)) })
            }
            "tool" => {
                let t = match de::<ToolIn>(&args) { Ok(r) => r, Err(e) => return Box::pin(async move { Err(e) }) };
                match hc.tools.iter().find(|(n, _, _)| *n == t.name) {
                    Some((_, _, f)) => {
                        let f = f(t.args, api);
                        Box::pin(async move { Ok(enc(&f.await)) })
                    }
                    None => Box::pin(async move { Ok(enc(&Err::<Value, String>(format!("unknown tool {}", t.name)))) }),
                }
            }
            "transport" => {
                let req = match de::<HttpReq>(&args) { Ok(r) => r, Err(e) => return Box::pin(async move { Err(e) }) };
                let Some(t) = hc.transport.clone() else { return Box::pin(async { Err("no transport registered".to_string()) }) };
                let (bodies, handle) = (self.bodies.clone(), self.next_body.fetch_add(1, SeqCst));
                let f = t(req, api);
                Box::pin(async move {
                    let (head, body) = f.await?;
                    bodies.lock().unwrap().insert(handle, Some(body));
                    Ok(enc(&TransportOut { head, body: handle }))
                })
            }
            "body_next" => {
                let h = u64::from_le_bytes(args[..8].try_into().unwrap());
                let bodies = self.bodies.clone();
                Box::pin(async move {
                    let taken = bodies.lock().unwrap().get_mut(&h).and_then(|s| s.take());
                    let Some(mut body) = taken else { return Ok(vec![0]) };
                    // if this future is dropped mid-await, `body` drops here: the socket closes
                    let c = body.next().await;
                    match c {
                        Some(c) => {
                            if let Some(slot) = bodies.lock().unwrap().get_mut(&h) {
                                *slot = Some(body);
                            }
                            let mut out = vec![1];
                            out.extend(c);
                            Ok(out)
                        }
                        None => Ok(vec![0]),
                    }
                })
            }
            other => {
                let m = format!("unknown op {other}");
                Box::pin(async move { Err(m) })
            }
        }
    }
    fn validate(&self, v: &[u8]) -> Result<(), String> {
        (self.hc.validate)(&de::<Value>(v)?)
    }
    fn inbox(&self) -> Vec<u8> {
        let items = self.inbox.drain();
        if items.is_empty() { vec![] } else { enc(&items) }
    }
    fn emit(&self, ev: &[u8]) {
        if let Ok(e) = de::<Ev>(ev) {
            (self.observer)(e)
        }
    }
    fn close(&self, _kind: &str, handle: u64) {
        self.bodies.lock().unwrap().remove(&handle);
    }
}

/// B2's meter: wraps the wire (library -> host calls) and the plain callbacks it hands out
/// (host -> library calls), counting each call and the bytes it carried.
pub struct CountingWire {
    pub inner: HostWire,
    pub meter: Arc<Meter>,
}
impl Wire for CountingWire {
    fn call(&self, op: &str, args: Vec<u8>, plain: Plain) -> BoxFut<Result<Vec<u8>, String>> {
        let name: &'static str = match op { "model" => "call:model", "tool" => "call:tool", "transport" => "call:transport", _ => "call:body_next" };
        self.meter.cross(name, args.len());
        let (m1, m2) = (self.meter.clone(), self.meter.clone());
        let (c, e) = (plain.cancelled.clone(), plain.emit.clone());
        let plain = Plain {
            cancelled: Arc::new(move || {
                m1.cross("plain:cancelled", 1);
                c()
            }),
            emit: Arc::new(move |b| {
                m2.cross("plain:emit", b.len());
                e(b)
            }),
        };
        let f = self.inner.call(op, args, plain);
        let m = self.meter.clone();
        Box::pin(async move {
            let r = f.await;
            m.cross("call:reply", r.as_ref().map(|b| b.len()).unwrap_or(0));
            r
        })
    }
    fn validate(&self, v: &[u8]) -> Result<(), String> {
        self.meter.cross("validate", v.len());
        self.inner.validate(v)
    }
    fn inbox(&self) -> Vec<u8> {
        let b = self.inner.inbox();
        self.meter.cross("inbox", b.len());
        b
    }
    fn emit(&self, ev: &[u8]) {
        self.meter.cross("wire:emit", ev.len());
        self.inner.emit(ev)
    }
    fn close(&self, kind: &str, handle: u64) {
        self.meter.cross("close", 8);
        self.inner.close(kind, handle)
    }
}
