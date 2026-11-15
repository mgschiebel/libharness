//! `Link`: everything the Loop needs from the host's callbacks, with no handle types in the
//! signatures (the token, sink and body live inside each implementation). The Loop (`drive`)
//! and the Exchange stand-in (`exchange`) are written once against it.
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::string::ToString;
use call_core::*;
use core::future::Future;
use core::pin::Pin;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use spin::Mutex;

pub type BoxFutR<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a host observer sees: live text deltas, and the core's events.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Ev {
    Delta(String),
    Core(Event),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpReq {
    pub url: String,
    pub body: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpHead {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

/// A response body. Pull a chunk, `None` at the end; dropping it must release the connection
/// (that is how a Transport call is cancelled). Note `next` borrows `self`: the body is lent
/// to the future, as the #34 surprise said it must be.
pub trait BodyStream: Send {
    fn next<'a>(&'a mut self) -> BoxFutR<'a, Option<Vec<u8>>>;
}

pub trait Link: Send + Sync {
    fn model(&self, req: ModelRequest) -> BoxFut<Result<ModelReply, String>>;
    fn tool(&self, call: ToolCall) -> BoxFut<Result<Value, String>>;
    fn transport(&self, req: HttpReq) -> BoxFut<Result<(HttpHead, Box<dyn BodyStream>), String>>;
    fn validate(&self, v: &Value) -> Result<(), String>;
    fn inbox(&self) -> Vec<(Inject, String)>;
    fn emit(&self, ev: &Ev);
    fn cancel(&self) -> &CancelToken;
}

/// The host's way to steer a run: a queue it holds a clone of. (Same in every variant: this
/// is the host calling the library, not a callback.)
#[derive(Clone, Default)]
pub struct Inbox(alloc::sync::Arc<Mutex<Vec<(Inject, String)>>>);
impl Inbox {
    pub fn steer(&self, t: impl Into<String>) {
        self.0.lock().push((Inject::Steer, t.into()));
    }
    pub fn follow_up(&self, t: impl Into<String>) {
        self.0.lock().push((Inject::FollowUp, t.into()));
    }
    pub fn drain(&self) -> Vec<(Inject, String)> {
        core::mem::take(&mut *self.0.lock())
    }
}

fn sync(link: &dyn Link, mut s: State) -> State {
    for (k, t) in link.inbox() {
        s = match k {
            Inject::Steer => steer(s, t),
            Inject::FollowUp => follow_up(s, t),
        };
    }
    s
}

/// Cancel wins over a result that landed in the extra poll (the third part of the contract).
fn settled<T>(tok: &CancelToken, r: Option<T>) -> Option<T> {
    if tok.is_cancelled() { None } else { r }
}

async fn model_call(link: &dyn Link, req: ModelRequest) -> Option<Result<ModelReply, String>> {
    let tok = link.cancel();
    settled(tok, abortable(tok, link.model(req)).await)
}

async fn tool_calls(link: &dyn Link, calls: &[ToolCall]) -> Option<Vec<ToolDone>> {
    let tok = link.cancel();
    let futs: Vec<BoxFut<ToolDone>> = calls
        .iter()
        .map(|c| {
            let id = c.id.clone();
            let f = link.tool(c.clone()); // eager: every tool of the batch starts now
            Box::pin(async move { ToolDone { call_id: id, result: f.await } }) as BoxFut<ToolDone>
        })
        .collect();
    settled(tok, abortable(tok, join_all(futs)).await)
}

/// The Loop: the shipped loop of ticket 35 minus the shell, as ADR 0004 left it.
pub async fn drive(link: &dyn Link, spec: &Spec, state: State) -> (State, Result<Value, SessionError>) {
    let mut out = resume(sync(link, state), spec);
    loop {
        for e in &out.events {
            link.emit(&Ev::Core(e.clone()));
        }
        if link.cancel().is_cancelled() && !matches!(out.step, Step::End(_)) {
            out = abort(out.state);
        }
        out = match out.step {
            Step::Call(req) => {
                let r = model_call(link, req).await;
                let st = sync(link, out.state);
                match r {
                    Some(r) => absorb(st, spec, &|v| link.validate(v), r),
                    None => abort(st),
                }
            }
            Step::Tools(calls) => {
                let d = tool_calls(link, &calls).await;
                let st = sync(link, out.state);
                match d {
                    Some(d) => tools_done(st, spec, d),
                    None => abort(st),
                }
            }
            Step::End(r) => return (out.state, r),
        };
    }
}

/// The Exchange stand-in: one model call over a Transport. Sends the request, reads the body
/// as `data: {"t":"<text>"}` lines (chunks may split anywhere), forwards each delta to the
/// host's sink as it arrives, ends at `data: [DONE]`. `None` = cancelled, in which case the
/// body has been dropped (and so closed) by the time this returns.
pub async fn exchange(link: &dyn Link, req: HttpReq) -> Option<Result<ModelReply, String>> {
    let tok = link.cancel();
    settled(tok, abortable(tok, exchange_inner(link, req)).await)
}

async fn exchange_inner(link: &dyn Link, req: HttpReq) -> Result<ModelReply, String> {
    let (head, mut body) = link.transport(req).await?;
    if head.status != 200 {
        return Err(format!("http {}", head.status));
    }
    let (mut buf, mut text, mut n) = (Vec::<u8>::new(), String::new(), 0u64);
    'read: while let Some(chunk) = body.next().await {
        buf.extend_from_slice(&chunk);
        while let Some(i) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=i).collect();
            let line = String::from_utf8_lossy(&line);
            let Some(data) = line.trim().strip_prefix("data: ") else { continue };
            if data == "[DONE]" {
                break 'read;
            }
            let v: Value = serde_json::from_str(data).map_err(|e| e.to_string())?;
            if let Some(t) = v["t"].as_str() {
                text.push_str(t);
                n += 1;
                link.emit(&Ev::Delta(t.to_string()));
            }
        }
    }
    Ok(ModelReply { text, tool_calls: Vec::new(), tokens: n })
}
