//! The pure half: session policy on top of the sans-IO core. Synchronous, never reads a
//! clock, never awaits. Translates session types <-> core types, resolves `final_answer`
//! in-process (typed result, bounded retries), nudges, counts turns. The async driver in
//! session.rs only moves values between this and the registered callbacks.
use crate::*;
use sans_io_core as sio;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

pub const FINAL_ANSWER: &str = "final_answer";

/// What the driver must do next.
pub enum Action<R> {
    CallModel { call_id: u64, messages: Vec<Message> },
    RunTool { id: String, name: String, args: Value },
    Event(Event),
    /// Terminal: exactly once, then `poll` returns None forever.
    Finish(Result<R, SessionError>),
}

pub struct Engine<R> {
    agent: sio::Agent,
    cfg: SessionConfig,
    schema: Value,
    now: Instant,
    out: VecDeque<Action<R>>,
    finished: bool,
    answer: Option<R>,
    failed: Option<String>,
    rejected: u32,
    nudges: u32,
    turns: u32,
    queued: (usize, usize),
    tool_names: HashMap<String, String>,
}

impl<R: serde::de::DeserializeOwned> Engine<R> {
    pub fn new(cfg: SessionConfig, result_schema: Value, tool_timeout: Option<Duration>, now: Instant) -> Self {
        Engine {
            agent: sio::Agent::new(sio::Config { tool_timeout }),
            cfg, schema: result_schema, now, out: VecDeque::new(), finished: false,
            answer: None, failed: None, rejected: 0, nudges: 0, turns: 0, queued: (0, 0), tool_names: HashMap::new(),
        }
    }

    // ---- inputs (every one carries `now`) -------------------------------------
    pub fn start(&mut self, now: Instant, prompt: String) {
        self.now = now;
        let _ = self.agent.prompt(now, prompt);
    }
    pub fn steer(&mut self, now: Instant, text: String) {
        self.now = now;
        if !self.finished {
            self.agent.steer(now, text);
        }
    }
    pub fn follow_up(&mut self, now: Instant, text: String) {
        self.now = now;
        if !self.finished {
            self.agent.follow_up(now, text);
        }
    }
    pub fn model_done(&mut self, now: Instant, call_id: u64, reply: Result<ModelReply, String>) {
        self.now = now;
        if self.finished {
            return;
        }
        match reply {
            Ok(r) => {
                self.out.push_back(Action::Event(Event::Tokens { session: self.cfg.id, tokens: r.tokens }));
                if !r.text.is_empty() {
                    self.agent.handle_model_event(now, call_id, sio::ModelEvent::TextDelta(r.text));
                }
                let stop = if r.tool_calls.is_empty() { sio::StopReason::EndTurn } else { sio::StopReason::ToolUse };
                for c in r.tool_calls {
                    let call = sio::ToolCall { id: c.id, name: c.name, args: c.args.to_string() };
                    self.agent.handle_model_event(now, call_id, sio::ModelEvent::ToolCall(call));
                }
                self.agent.handle_model_event(now, call_id, sio::ModelEvent::Done(stop));
            }
            Err(msg) => {
                self.failed = Some(msg.clone());
                self.agent.handle_model_event(now, call_id, sio::ModelEvent::Failed(msg));
            }
        }
    }
    pub fn tool_done(&mut self, now: Instant, id: &str, res: Result<Value, String>) {
        self.now = now;
        if self.finished {
            return;
        }
        let (content, is_error) = match res {
            Ok(v) => (v.to_string(), false),
            Err(s) => (s, true),
        };
        self.agent.handle_tool_result(now, id, sio::ToolResult { content, is_error });
    }
    pub fn timeout(&mut self, now: Instant) {
        self.now = now;
        self.agent.handle_timeout(now);
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.agent.poll_timeout()
    }
    pub fn abort(&mut self, now: Instant) {
        self.now = now;
        if !self.finished {
            self.agent.abort(now);
            self.out.clear();
            self.finish(Err(SessionError::Aborted));
        }
    }

    // ---- outputs ----------------------------------------------------------------
    pub fn poll(&mut self) -> Option<Action<R>> {
        loop {
            if let Some(a) = self.out.pop_front() {
                return Some(a);
            }
            if self.finished {
                return None;
            }
            let o = self.agent.poll_output()?;
            self.translate(o);
        }
    }

    // ---- internals --------------------------------------------------------------
    fn finish(&mut self, r: Result<R, SessionError>) {
        self.finished = true;
        self.out.push_back(Action::Finish(r));
    }

    fn translate(&mut self, o: sio::Output) {
        match o {
            sio::Output::CallModel(req) => {
                self.out.push_back(Action::CallModel { call_id: req.call_id, messages: to_session(&req.messages) });
            }
            // Advisory in the core. Here the driver drops the in-flight future instead.
            sio::Output::CancelModel { .. } | sio::Output::CancelTool { .. } => {}
            sio::Output::RunTool(c) if c.name == FINAL_ANSWER => self.final_answer(c),
            sio::Output::RunTool(c) => {
                let args = serde_json::from_str(&c.args).unwrap_or(Value::Null);
                self.out.push_back(Action::RunTool { id: c.id, name: c.name, args });
            }
            sio::Output::Event(e) => self.event(e),
        }
    }

    fn event(&mut self, e: sio::AgentEvent) {
        use sio::AgentEvent as E;
        let session = self.cfg.id;
        match e {
            E::TurnStart => {
                if self.turns >= self.cfg.max_turns {
                    return self.finish(Err(SessionError::MaxTurns));
                }
                self.out.push_back(Action::Event(Event::TurnStart { session, turn: self.turns }));
                self.turns += 1;
            }
            E::ToolExecutionStart { id, name, .. } => {
                if name != FINAL_ANSWER {
                    self.out.push_back(Action::Event(Event::ToolStart { session, name: name.clone() }));
                }
                self.tool_names.insert(id, name);
            }
            E::ToolExecutionEnd { id, is_error, .. } => {
                let name = self.tool_names.remove(&id).unwrap_or_default();
                if name != FINAL_ANSWER {
                    self.out.push_back(Action::Event(Event::ToolEnd { session, name, is_error }));
                }
            }
            E::QueueUpdate { steering, follow_up } => {
                if steering < self.queued.0 {
                    self.out.push_back(Action::Event(Event::Injected { session, kind: "steer" }));
                }
                if follow_up < self.queued.1 {
                    self.out.push_back(Action::Event(Event::Injected { session, kind: "follow_up" }));
                }
                self.queued = (steering, follow_up);
            }
            E::AgentEnd { aborted: false } => self.run_ended(),
            _ => {}
        }
    }

    /// The core went idle: no tools, no queued input. Decide whether that is the end.
    fn run_ended(&mut self) {
        if let Some(msg) = self.failed.take() {
            return self.finish(Err(SessionError::ModelFailed(msg)));
        }
        if let Some(a) = self.answer.take() {
            return self.finish(Ok(a));
        }
        if self.nudges < self.cfg.max_nudges {
            self.nudges += 1;
            let _ = self.agent.prompt(self.now, format!("Finish by calling {FINAL_ANSWER}."));
        } else {
            self.finish(Err(SessionError::NoAnswer));
        }
    }

    /// `final_answer` never leaves the engine: parse it as `R`, accept or bounce it.
    fn final_answer(&mut self, c: sio::ToolCall) {
        let args: Value = serde_json::from_str(&c.args).unwrap_or(Value::Null);
        let raw = args.get("result").cloned().unwrap_or(Value::Null);
        let (content, is_error) = match serde_json::from_value::<R>(raw.clone()) {
            Ok(v) => {
                self.answer = Some(v);
                self.agent.mark_done(); // the one core patch: stop after this batch
                ("accepted".to_string(), false)
            }
            Err(e) => {
                self.rejected += 1;
                self.out.push_back(Action::Event(Event::ResultRejected { session: self.cfg.id, error: e.to_string() }));
                if self.rejected > self.cfg.max_result_retries {
                    return self.finish(Err(SessionError::Malformed { attempts: self.rejected, last_error: e.to_string(), raw }));
                }
                (format!("rejected: {e}. Expected schema: {}", self.schema), true)
            }
        };
        self.agent.handle_tool_result(self.now, &c.id, sio::ToolResult { content, is_error });
    }
}

fn to_session(msgs: &[sio::Message]) -> Vec<Message> {
    let mut names: HashMap<&str, &str> = HashMap::new();
    msgs.iter()
        .map(|m| match m {
            sio::Message::User(s) => Message::User(s.clone()),
            sio::Message::Assistant { text, tool_calls, .. } => {
                for c in tool_calls {
                    names.insert(&c.id, &c.name);
                }
                let tool_calls = tool_calls
                    .iter()
                    .map(|c| ToolCall { id: c.id.clone(), name: c.name.clone(), args: serde_json::from_str(&c.args).unwrap_or(Value::Null) })
                    .collect();
                Message::Assistant { text: text.clone(), tool_calls }
            }
            sio::Message::ToolResult { tool_call_id, content, is_error } => Message::ToolResult {
                call_id: tool_call_id.clone(),
                name: names.get(tool_call_id.as_str()).copied().unwrap_or("").to_string(),
                content: content.clone(),
                is_error: *is_error,
            },
        })
        .collect()
}
