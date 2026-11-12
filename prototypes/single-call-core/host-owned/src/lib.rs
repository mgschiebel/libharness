//! OWNER 1: the host's own plain code. No executor, no futures, no async, no callback
//! registry: ordinary functions and blocking calls, with the core's pure transitions.
//! This is what a host writes for steering, abort and a step hook (checkpoint, budget).
use call_core::*;
use serde_json::Value;

/// Input the host's own mailbox yields (the host decides where it comes from).
pub enum Input {
    Steer(String),
    FollowUp(String),
}

pub struct Wiring<'a> {
    pub spec: &'a Spec,
    pub validate: &'a dyn Fn(&Value) -> Result<(), String>,
    pub cancel: &'a CancelToken,
    pub model: &'a mut dyn FnMut(ModelRequest, &CancelToken) -> Result<ModelReply, String>,
    pub tool: &'a mut dyn FnMut(&ToolCall, &CancelToken) -> Result<Value, String>,
    pub mailbox: &'a mut dyn FnMut() -> Vec<Input>,
    /// Called before every step with the state; return false to stop (budget, kill switch).
    pub on_step: &'a mut dyn FnMut(&State) -> bool,
}

pub fn run(w: &mut Wiring, state: State) -> (State, Result<Value, SessionError>) {
    let mut out = resume(apply((w.mailbox)(), state), w.spec);
    loop {
        if !matches!(out.step, Step::End(_)) && (w.cancel.is_cancelled() || !(w.on_step)(&out.state)) {
            out = abort(out.state);
        }
        out = match out.step {
            Step::Call(req) => {
                let reply = (w.model)(req, w.cancel);
                if w.cancel.is_cancelled() {
                    abort(out.state)
                } else {
                    absorb(apply((w.mailbox)(), out.state), w.spec, w.validate, reply)
                }
            }
            Step::Tools(calls) => {
                let done: Vec<ToolDone> = calls.iter().map(|c| ToolDone { call_id: c.id.clone(), result: (w.tool)(c, w.cancel) }).collect();
                if w.cancel.is_cancelled() {
                    abort(out.state)
                } else {
                    tools_done(apply((w.mailbox)(), out.state), w.spec, done)
                }
            }
            Step::End(r) => return (out.state, r),
        };
    }
}

fn apply(inputs: Vec<Input>, s: State) -> State {
    inputs.into_iter().fold(s, |s, i| match i {
        Input::Steer(t) => steer(s, t),
        Input::FollowUp(t) => follow_up(s, t),
    })
}
