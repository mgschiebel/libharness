//! The pure half: `(State, input) -> Out`. No async, no callbacks, no clock, no threads.
use crate::*;
use alloc::format;
use alloc::string::ToString;
use alloc::vec;
use serde_json::json;

pub fn start(id: SessionId, cfg: Config, prompt: impl Into<String>) -> State {
    State {
        id,
        cfg,
        phase: Phase::Ready,
        messages: vec![Message::User(prompt.into())],
        steer: vec![],
        follow: vec![],
        pending: vec![],
        answer: None,
        turn: 0,
        rejected: 0,
        nudges: 0,
    }
}

/// Queue input. Delivered at the next turn boundary / when the run would stop. The loop
/// owner applies these: while a call is in flight the state is owned by the loop.
pub fn steer(mut s: State, text: impl Into<String>) -> State {
    s.steer.push(text.into());
    s
}
pub fn follow_up(mut s: State, text: impl Into<String>) -> State {
    s.follow.push(text.into());
    s
}

/// First transition: `Ready -> AwaitingModel`.
pub fn begin(s: State, spec: &Spec) -> Out {
    if s.phase != Phase::Ready {
        return protocol(s, "begin outside Ready");
    }
    next(s, spec, vec![])
}

/// "Given this state, what is it waiting for?" Works on any state, including one that
/// was serialised, stored and loaded elsewhere. Re-asks the model call / re-runs the
/// tool batch that was in flight when the state was taken.
pub fn resume(s: State, spec: &Spec) -> Out {
    match s.phase {
        Phase::Ready => begin(s, spec),
        Phase::AwaitingModel => {
            let req = request(&s, spec);
            Out { state: s, step: Step::Call(req), events: vec![] }
        }
        Phase::AwaitingTools => {
            let run = s.pending.iter().filter(|p| p.outcome.is_none()).map(|p| p.call.clone()).collect();
            Out { state: s, step: Step::Tools(run), events: vec![] }
        }
        Phase::Done => protocol(s, "resume on a finished run"),
    }
}

/// Feed the model callback's result. Interprets the reply: `final_answer` is validated
/// here (typed result, bounded retry), unknown tools are answered here, the rest is handed out.
pub fn absorb(
    mut s: State,
    spec: &Spec,
    validate: &dyn Fn(&Value) -> Result<(), String>,
    reply: Result<ModelReply, String>,
) -> Out {
    if s.phase != Phase::AwaitingModel {
        return protocol(s, "absorb outside a model call");
    }
    let mut ev = vec![];
    let reply = match reply {
        Ok(r) => r,
        Err(e) => return end(s, Err(SessionError::ModelFailed(e)), ev),
    };
    ev.push(Event::Tokens { session: s.id, tokens: reply.tokens });
    s.messages.push(Message::Assistant { text: reply.text.clone(), tool_calls: reply.tool_calls.clone() });

    if reply.tool_calls.is_empty() {
        if take_follow(&mut s, &mut ev) {
            return next(s, spec, ev);
        }
        if let Some(a) = s.answer.take() {
            return end(s, Ok(a), ev);
        }
        if s.nudges < s.cfg.max_nudges {
            s.nudges += 1;
            s.messages.push(Message::User(format!("Finish by calling {FINAL_ANSWER}.")));
            return next(s, spec, ev);
        }
        return end(s, Err(SessionError::NoAnswer), ev);
    }

    let mut run = vec![];
    for call in reply.tool_calls {
        let outcome = if call.name == FINAL_ANSWER {
            let raw = call.args.get("result").cloned().unwrap_or(Value::Null);
            match validate(&raw) {
                Ok(()) => {
                    s.answer = Some(raw);
                    Some(("accepted".to_string(), false))
                }
                Err(e) => {
                    s.rejected += 1;
                    ev.push(Event::ResultRejected { session: s.id, error: e.clone() });
                    if s.rejected > s.cfg.max_result_retries {
                        let err = SessionError::Malformed { attempts: s.rejected, last_error: e, raw };
                        return end(s, Err(err), ev);
                    }
                    Some((format!("rejected: {e}. Expected schema: {}", spec.result_schema), true))
                }
            }
        } else {
            ev.push(Event::ToolStart { session: s.id, name: call.name.clone() });
            if spec.tools.iter().any(|t| t.name == call.name) {
                run.push(call.clone());
                None
            } else {
                Some((format!("unknown tool {}", call.name), true))
            }
        };
        s.pending.push(Pending { call, outcome });
    }
    s.phase = Phase::AwaitingTools;
    if run.is_empty() {
        return flush(s, spec, ev);
    }
    Out { state: s, step: Step::Tools(run), events: ev }
}

/// Feed the results of the `Step::Tools` batch (any order; matched by call id).
pub fn tools_done(mut s: State, spec: &Spec, results: Vec<ToolDone>) -> Out {
    if s.phase != Phase::AwaitingTools {
        return protocol(s, "tools_done outside a tool batch");
    }
    for r in results {
        let (content, is_error) = match r.result {
            Ok(v) => (v.to_string(), false),
            Err(e) => (e, true),
        };
        if let Some(p) = s.pending.iter_mut().find(|p| p.call.id == r.call_id && p.outcome.is_none()) {
            p.outcome = Some((content, is_error));
        }
    }
    flush(s, spec, vec![])
}

/// Terminal transition from any phase. Idempotent.
pub fn abort(s: State) -> Out {
    end(s, Err(SessionError::Aborted), vec![])
}

// ---- internals ---------------------------------------------------------------
fn flush(mut s: State, spec: &Spec, mut ev: Vec<Event>) -> Out {
    for p in core::mem::take(&mut s.pending) {
        let (content, is_error) = p.outcome.unwrap_or(("no result supplied".to_string(), true));
        if p.call.name != FINAL_ANSWER {
            ev.push(Event::ToolEnd { session: s.id, name: p.call.name.clone(), is_error });
        }
        s.messages.push(Message::ToolResult { call_id: p.call.id, name: p.call.name, content, is_error });
    }
    // Accepted answer: stop unless input is queued (a queued steer keeps the run going;
    // a follow-up is injected because the run would otherwise end).
    if s.answer.is_some() && s.steer.is_empty() {
        if take_follow(&mut s, &mut ev) {
            return next(s, spec, ev);
        }
        let a = s.answer.take().unwrap();
        return end(s, Ok(a), ev);
    }
    next(s, spec, ev)
}

/// Turn boundary: deliver steers, build the next request.
fn next(mut s: State, spec: &Spec, mut ev: Vec<Event>) -> Out {
    if s.turn >= s.cfg.max_turns {
        return end(s, Err(SessionError::MaxTurns), ev);
    }
    for t in core::mem::take(&mut s.steer) {
        ev.push(Event::Injected { session: s.id, kind: Inject::Steer });
        s.messages.push(Message::User(t));
    }
    ev.push(Event::TurnStart { session: s.id, turn: s.turn });
    s.turn += 1;
    s.phase = Phase::AwaitingModel;
    let req = request(&s, spec);
    Out { state: s, step: Step::Call(req), events: ev }
}

fn request(s: &State, spec: &Spec) -> ModelRequest {
    let mut tools = spec.tools.clone();
    tools.push(ToolSpec {
        name: FINAL_ANSWER.into(),
        description: "Call exactly when done, with the result in the requested shape.".into(),
        params: json!({"type":"object","properties":{"result": spec.result_schema},"required":["result"]}),
    });
    ModelRequest { session: s.id, messages: s.messages.clone(), tools }
}

fn take_follow(s: &mut State, ev: &mut Vec<Event>) -> bool {
    if s.follow.is_empty() {
        return false;
    }
    let f = s.follow.remove(0);
    ev.push(Event::Injected { session: s.id, kind: Inject::FollowUp });
    s.messages.push(Message::User(f));
    true
}
fn end(mut s: State, r: Result<Value, SessionError>, events: Vec<Event>) -> Out {
    s.phase = Phase::Done;
    s.pending.clear();
    Out { state: s, step: Step::End(r), events }
}
fn protocol(s: State, what: &str) -> Out {
    let p = s.phase;
    end(s, Err(SessionError::Protocol(format!("{what} (phase {p:?})"))), vec![])
}
