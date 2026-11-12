//! Optional async helpers over the registry. Still no runtime: `core::future` only.
//! This is where the "poll once more after cancel" contract lives.
use crate::*;
use alloc::boxed::Box;
use alloc::format;
use alloc::vec::Vec;
use core::future::{poll_fn, Future};
use core::pin::pin;
use core::task::Poll;

/// Poll `f`; if the token fires first, drop `f` and return None. Contract: `f` is polled
/// once more after cancel() (hooks have already run synchronously), so a callback that
/// awaits a child sees the child's abort before being dropped. Results win races.
pub async fn abortable<F: Future>(tok: &CancelToken, f: F) -> Option<F::Output> {
    let mut f = pin!(f);
    let mut c = pin!(tok.cancelled());
    poll_fn(|cx| {
        if let Poll::Ready(o) = f.as_mut().poll(cx) {
            return Poll::Ready(Some(o));
        }
        match c.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

/// Minimal join_all so the core needs no futures crate.
pub async fn join_all<T>(futs: Vec<BoxFut<T>>) -> Vec<T> {
    let mut slots: Vec<(Option<BoxFut<T>>, Option<T>)> = futs.into_iter().map(|f| (Some(f), None)).collect();
    poll_fn(|cx| {
        let mut pending = false;
        for (f, out) in slots.iter_mut() {
            if let Some(fut) = f {
                match fut.as_mut().poll(cx) {
                    Poll::Ready(v) => {
                        *out = Some(v);
                        *f = None;
                    }
                    Poll::Pending => pending = true,
                }
            }
        }
        if pending { Poll::Pending } else { Poll::Ready(()) }
    })
    .await;
    slots.into_iter().map(|(_, o)| o.unwrap()).collect()
}

impl Callbacks {
    /// THE single call: invoke the registered model callback once. `None` = aborted.
    pub async fn call_model(&self, req: ModelRequest, tok: &CancelToken) -> Option<Result<ModelReply, String>> {
        let Some(m) = &self.model else { return Some(Err("no model registered".into())) };
        settled(tok, abortable(tok, m(req, tok.clone())).await)
    }
    /// Run a `Step::Tools` batch concurrently inside the caller's task. `None` = aborted.
    pub async fn call_tools(&self, session: SessionId, calls: &[ToolCall], tok: &CancelToken) -> Option<Vec<ToolDone>> {
        let futs: Vec<BoxFut<ToolDone>> = calls
            .iter()
            .map(|c| {
                let id = c.id.clone();
                let fut: BoxFut<Result<Value, String>> = match self.tools.iter().find(|(n, _)| *n == c.name) {
                    Some((_, f)) => f(ToolCtx { session, call_id: id.clone(), cancel: tok.clone() }, c.args.clone()),
                    None => {
                        let m = format!("unknown tool {}", c.name);
                        Box::pin(async move { Err(m) })
                    }
                };
                Box::pin(async move { ToolDone { call_id: id, result: fut.await } }) as BoxFut<ToolDone>
            })
            .collect();
        settled(tok, abortable(tok, join_all(futs)).await)
    }
}

/// Cancel wins over a result that landed in the extra poll: a run never continues past a
/// callback after abort. (Found by the first cut of the host's own loop, which continued.)
fn settled<T>(tok: &CancelToken, r: Option<T>) -> Option<T> {
    if tok.is_cancelled() { None } else { r }
}
