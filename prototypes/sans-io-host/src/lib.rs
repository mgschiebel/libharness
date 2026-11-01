//! PROTOTYPE (throwaway, libharness#6): a sans-IO agent loop.
//!
//! No I/O, no async, no clock reads. The host feeds inputs (each with an explicit `now`)
//! and drains outputs with `poll_output()`. Event vocabulary mirrors Pi's `AgentEvent`
//! (research/pi-sdk.md section 3). Loop shape mirrors Pi's `runLoop` (section 2):
//! steering is polled after a turn's tool batch finishes, follow-ups only when the
//! agent would otherwise stop.
//!
//! Drain rule (str0m style): after every input call, drain `poll_output()` until `None`
//! before the next input. (Not enforced here; see README.)

use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub type CallId = u64;

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    Length,
    Error,
    Aborted,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    User(String),
    Assistant { text: String, tool_calls: Vec<ToolCall>, stop: StopReason },
    ToolResult { tool_call_id: String, content: String, is_error: bool },
}

/// Host -> core: what the model produced (host translates SSE chunks into these).
#[derive(Clone, Debug, PartialEq)]
pub enum ModelEvent {
    TextDelta(String),
    /// A complete tool call (streaming arg deltas deliberately out of scope: unverified in openrouter-api.md).
    ToolCall(ToolCall),
    Done(StopReason),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelRequest {
    pub call_id: CallId,
    pub messages: Vec<Message>,
}

/// Pi-style events (subset).
#[derive(Clone, Debug, PartialEq)]
pub enum AgentEvent {
    AgentStart,
    AgentEnd { aborted: bool },
    TurnStart,
    TurnEnd { tool_results: usize },
    MessageStart { role: &'static str },
    MessageUpdate { delta: String },
    MessageEnd { message: Message },
    ToolExecutionStart { id: String, name: String, args: String },
    ToolExecutionEnd { id: String, is_error: bool, duration: Duration },
    QueueUpdate { steering: usize, follow_up: usize },
}

/// Core -> host: things to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Output {
    CallModel(ModelRequest),
    CancelModel { call_id: CallId },
    RunTool(ToolCall),
    CancelTool { tool_call_id: String },
    Event(AgentEvent),
}

#[derive(Debug, PartialEq)]
pub struct Busy;

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub tool_timeout: Option<Duration>,
}

struct Slot {
    call: ToolCall,
    started: Instant,
    deadline: Option<Instant>,
    result: Option<ToolResult>,
}

enum Phase {
    Idle,
    Model { call_id: CallId },
    Tools { slots: Vec<Slot> },
}

pub struct Agent {
    cfg: Config,
    messages: Vec<Message>,
    phase: Phase,
    out: VecDeque<Output>,
    steering: VecDeque<String>,
    follow_up: VecDeque<String>,
    pending: Vec<Message>,
    next_call: CallId,
    cur_text: String,
    cur_calls: Vec<ToolCall>,
    assistant_started: bool,
}

impl Agent {
    pub fn new(cfg: Config) -> Self {
        Agent {
            cfg,
            messages: vec![],
            phase: Phase::Idle,
            out: VecDeque::new(),
            steering: VecDeque::new(),
            follow_up: VecDeque::new(),
            pending: vec![],
            next_call: 0,
            cur_text: String::new(),
            cur_calls: vec![],
            assistant_started: false,
        }
    }

    // ---------- inputs ----------

    /// Start a run. Errors if a run is active (Pi's session layer: must choose steer/follow-up).
    pub fn prompt(&mut self, now: Instant, text: impl Into<String>) -> Result<(), Busy> {
        if !self.is_idle() {
            return Err(Busy);
        }
        self.out.push_back(Output::Event(AgentEvent::AgentStart));
        self.pending.push(Message::User(text.into()));
        self.start_turn(now);
        Ok(())
    }

    /// Deliver after the current turn's tool batch finishes. Idle => behaves like prompt.
    pub fn steer(&mut self, now: Instant, text: impl Into<String>) {
        if self.is_idle() {
            let _ = self.prompt(now, text);
            return;
        }
        self.steering.push_back(text.into());
        self.queue_update();
    }

    /// Deliver only when the agent would otherwise stop. Idle => behaves like prompt.
    pub fn follow_up(&mut self, now: Instant, text: impl Into<String>) {
        if self.is_idle() {
            let _ = self.prompt(now, text);
            return;
        }
        self.follow_up.push_back(text.into());
        self.queue_update();
    }

    pub fn handle_model_event(&mut self, _now: Instant, call_id: CallId, ev: ModelEvent) {
        // Stale/duplicate inputs (e.g. after abort) are silently ignored.
        match self.phase {
            Phase::Model { call_id: c } if c == call_id => {}
            _ => return,
        }
        match ev {
            ModelEvent::TextDelta(d) => {
                self.ensure_assistant_started();
                self.cur_text.push_str(&d);
                self.emit(AgentEvent::MessageUpdate { delta: d });
            }
            ModelEvent::ToolCall(c) => self.cur_calls.push(c),
            ModelEvent::Done(stop) => self.finish_assistant(_now, stop),
            ModelEvent::Failed(msg) => {
                self.cur_text.push_str(&format!("[error: {msg}]"));
                self.cur_calls.clear();
                self.finish_assistant(_now, StopReason::Error);
            }
        }
    }

    pub fn handle_tool_result(&mut self, now: Instant, tool_call_id: &str, result: ToolResult) {
        let idx = match &self.phase {
            Phase::Tools { slots } => slots
                .iter()
                .position(|s| s.call.id == tool_call_id && s.result.is_none()),
            _ => None,
        };
        if let Some(i) = idx {
            self.complete_slot(now, i, result);
        }
    }

    /// Spurious calls are no-ops (quinn-proto contract).
    pub fn handle_timeout(&mut self, now: Instant) {
        let expired: Vec<usize> = match &self.phase {
            Phase::Tools { slots } => slots
                .iter()
                .enumerate()
                .filter(|(_, s)| s.result.is_none() && s.deadline.map_or(false, |d| d <= now))
                .map(|(i, _)| i)
                .collect(),
            _ => vec![],
        };
        for i in expired {
            let id = self.slot(i).call.id.clone();
            self.out.push_back(Output::CancelTool { tool_call_id: id });
            self.complete_slot(now, i, ToolResult { content: "tool timed out".into(), is_error: true });
        }
    }

    pub fn abort(&mut self, now: Instant) {
        let _ = now;
        match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Idle => return,
            Phase::Model { call_id } => {
                self.out.push_back(Output::CancelModel { call_id });
                if self.assistant_started {
                    let text = std::mem::take(&mut self.cur_text);
                    self.emit_message(Message::Assistant { text, tool_calls: vec![], stop: StopReason::Aborted });
                }
            }
            Phase::Tools { slots } => {
                // keep history valid: every tool call gets a result
                for s in slots {
                    let res = match s.result {
                        Some(r) => r,
                        None => {
                            self.out.push_back(Output::CancelTool { tool_call_id: s.call.id.clone() });
                            ToolResult { content: "aborted".into(), is_error: true }
                        }
                    };
                    self.messages.push(Message::ToolResult {
                        tool_call_id: s.call.id,
                        content: res.content,
                        is_error: res.is_error,
                    });
                }
            }
        }
        self.emit(AgentEvent::TurnEnd { tool_results: 0 });
        self.emit(AgentEvent::AgentEnd { aborted: true });
    }

    // ---------- outputs / queries ----------

    pub fn poll_output(&mut self) -> Option<Output> {
        self.out.pop_front()
    }

    /// Earliest tool deadline. Re-read after every call; it changes.
    pub fn poll_timeout(&self) -> Option<Instant> {
        match &self.phase {
            Phase::Tools { slots } => slots.iter().filter(|s| s.result.is_none()).filter_map(|s| s.deadline).min(),
            _ => None,
        }
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.phase, Phase::Idle)
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Pi returns queued messages to the editor on abort; here the host asks.
    pub fn take_queued(&mut self) -> (Vec<String>, Vec<String>) {
        let r = (self.steering.drain(..).collect(), self.follow_up.drain(..).collect());
        self.queue_update();
        r
    }

    // ---------- internals ----------

    fn slot(&self, i: usize) -> &Slot {
        match &self.phase {
            Phase::Tools { slots } => &slots[i],
            _ => unreachable!(),
        }
    }

    fn emit(&mut self, e: AgentEvent) {
        self.out.push_back(Output::Event(e));
    }

    fn queue_update(&mut self) {
        let e = AgentEvent::QueueUpdate { steering: self.steering.len(), follow_up: self.follow_up.len() };
        self.emit(e);
    }

    fn emit_message(&mut self, m: Message) {
        let role = match &m {
            Message::User(_) => "user",
            Message::Assistant { .. } => "assistant",
            Message::ToolResult { .. } => "toolResult",
        };
        self.emit(AgentEvent::MessageStart { role });
        self.messages.push(m.clone());
        self.emit(AgentEvent::MessageEnd { message: m });
    }

    fn ensure_assistant_started(&mut self) {
        if !self.assistant_started {
            self.assistant_started = true;
            self.emit(AgentEvent::MessageStart { role: "assistant" });
        }
    }

    fn start_turn(&mut self, _now: Instant) {
        self.emit(AgentEvent::TurnStart);
        for m in std::mem::take(&mut self.pending) {
            self.emit_message(m);
        }
        self.next_call += 1;
        let call_id = self.next_call;
        self.cur_text.clear();
        self.cur_calls.clear();
        self.assistant_started = false;
        self.phase = Phase::Model { call_id };
        self.out.push_back(Output::CallModel(ModelRequest { call_id, messages: self.messages.clone() }));
    }

    fn finish_assistant(&mut self, now: Instant, stop: StopReason) {
        self.ensure_assistant_started();
        let text = std::mem::take(&mut self.cur_text);
        let calls = std::mem::take(&mut self.cur_calls);
        self.messages.push(Message::Assistant { text: text.clone(), tool_calls: calls.clone(), stop });
        let msg = self.messages.last().unwrap().clone();
        self.emit(AgentEvent::MessageEnd { message: msg });
        self.assistant_started = false;

        if matches!(stop, StopReason::Error | StopReason::Aborted) {
            self.phase = Phase::Idle;
            self.emit(AgentEvent::TurnEnd { tool_results: 0 });
            self.emit(AgentEvent::AgentEnd { aborted: stop == StopReason::Aborted });
            return;
        }
        if calls.is_empty() {
            return self.after_turn(now, false, 0);
        }
        if stop == StopReason::Length {
            // Pi: args may be truncated, so fail rather than execute.
            let n = calls.len();
            for c in calls {
                self.emit_message(Message::ToolResult {
                    tool_call_id: c.id,
                    content: "output truncated; tool not run".into(),
                    is_error: true,
                });
            }
            return self.after_turn(now, true, n);
        }
        let deadline = self.cfg.tool_timeout.map(|t| now + t);
        let mut slots = vec![];
        for c in calls {
            self.emit(AgentEvent::ToolExecutionStart { id: c.id.clone(), name: c.name.clone(), args: c.args.clone() });
            self.out.push_back(Output::RunTool(c.clone()));
            slots.push(Slot { call: c, started: now, deadline, result: None });
        }
        self.phase = Phase::Tools { slots };
    }

    fn complete_slot(&mut self, now: Instant, i: usize, result: ToolResult) {
        let (id, started) = {
            let s = match &mut self.phase {
                Phase::Tools { slots } => &mut slots[i],
                _ => unreachable!(),
            };
            s.result = Some(result.clone());
            (s.call.id.clone(), s.started)
        };
        self.emit(AgentEvent::ToolExecutionEnd { id, is_error: result.is_error, duration: now - started });
        let all_done = matches!(&self.phase, Phase::Tools { slots } if slots.iter().all(|s| s.result.is_some()));
        if all_done {
            let slots = match std::mem::replace(&mut self.phase, Phase::Idle) {
                Phase::Tools { slots } => slots,
                _ => unreachable!(),
            };
            let n = slots.len();
            // tool-result messages in assistant source order (Pi)
            for s in slots {
                let r = s.result.unwrap();
                self.emit_message(Message::ToolResult { tool_call_id: s.call.id, content: r.content, is_error: r.is_error });
            }
            self.after_turn(now, true, n);
        }
    }

    fn after_turn(&mut self, now: Instant, had_tools: bool, n_results: usize) {
        self.emit(AgentEvent::TurnEnd { tool_results: n_results });
        let mut continue_run = had_tools;
        if let Some(s) = self.steering.pop_front() {
            self.pending.push(Message::User(s));
            continue_run = true;
            self.queue_update();
        } else if !continue_run {
            if let Some(f) = self.follow_up.pop_front() {
                self.pending.push(Message::User(f));
                continue_run = true;
                self.queue_update();
            }
        }
        if continue_run {
            self.start_turn(now);
        } else {
            self.phase = Phase::Idle;
            self.emit(AgentEvent::AgentEnd { aborted: false });
        }
    }
}
pub mod toy;
