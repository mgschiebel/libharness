//! OWNER 2: a loop the host wrote and registered as a callback (`caps.with_loop(..)`).
//! State in, state out. It uses the core's three transitions and the optional async
//! helpers; it does not forward events and does not check the token at boundaries (the
//! helpers already honour it). It adds one thing the shipped loop does not do: it
//! records the serialised state before every step (a host-side checkpoint).
use session_shell::*;
use std::sync::{Arc, Mutex};

pub type Checkpoints = Arc<Mutex<Vec<String>>>;

pub fn host_loop(store: Option<Checkpoints>) -> LoopFn {
    Arc::new(move |ctx, state| Box::pin(body(ctx, state, store.clone())))
}

async fn body(ctx: LoopCtx, state: State, store: Option<Checkpoints>) -> (State, Result<serde_json::Value, SessionError>) {
    let spec = &ctx.cb.spec;
    let mut out = resume(ctx.sync(state), spec);
    loop {
        if let Some(s) = &store {
            s.lock().unwrap().push(serde_json::to_string(&out.state).unwrap());
        }
        out = match out.step {
            Step::Call(req) => match ctx.cb.call_model(req, &ctx.cancel).await {
                Some(r) => absorb(ctx.sync(out.state), spec, &*ctx.cb.validate, r),
                None => abort(out.state),
            },
            Step::Tools(calls) => match ctx.cb.call_tools(out.state.id, &calls, &ctx.cancel).await {
                Some(d) => tools_done(ctx.sync(out.state), spec, d),
                None => abort(out.state),
            },
            Step::End(r) => return (out.state, r),
        };
    }
}
