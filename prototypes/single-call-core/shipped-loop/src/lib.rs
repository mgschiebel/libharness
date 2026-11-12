//! PROTOTYPE (throwaway): the optional shipped loop. session-task's turn loop (concurrent
//! tool batches, steer at the boundary, follow-up when it would stop, abort) rebuilt on the
//! core's three transitions. Registered like any host loop: `caps.with_loop(shipped())`.
#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use session_shell::*;

pub fn shipped() -> LoopFn {
    Arc::new(|ctx, state| Box::pin(run(ctx, state)))
}

async fn run(ctx: LoopCtx, state: State) -> (State, Result<serde_json::Value, SessionError>) {
    let spec = &ctx.cb.spec;
    let mut out = resume(ctx.sync(state), spec);
    loop {
        ctx.emit(&out.events);
        if ctx.cancel.is_cancelled() && !matches!(out.step, Step::End(_)) {
            out = abort(out.state); // checked at every boundary, like session-task's turn top
        }
        out = match out.step {
            Step::Call(req) => {
                let reply = ctx.cb.call_model(req, &ctx.cancel).await;
                let st = ctx.sync(out.state); // steer/follow that arrived during the call
                match reply {
                    Some(r) => absorb(st, spec, &*ctx.cb.validate, r),
                    None => abort(st),
                }
            }
            Step::Tools(calls) => {
                let done = ctx.cb.call_tools(out.state.id, &calls, &ctx.cancel).await;
                let st = ctx.sync(out.state);
                match done {
                    Some(d) => tools_done(st, spec, d),
                    None => abort(st),
                }
            }
            Step::End(r) => return (out.state, r),
        };
    }
}
