//! Toy host pieces shared by the sync and async hosts: fake model, fake tools, scenarios.
//! Plans are plain data (delay_ms, payload) so each host plays them with its own sleep.

use crate::*;
use std::time::Instant;

#[derive(Clone, Debug)]
pub enum Cmd {
    Prompt(&'static str),
    Steer(&'static str),
    FollowUp(&'static str),
    Abort,
}

/// Everything a host loop waits on, funnelled into one channel.
#[derive(Debug)]
pub enum HostMsg {
    Cmd(Cmd),
    Model(CallId, ModelEvent),
    Tool(String, ToolResult),
    ScriptDone,
}

/// Rule-based fake model: decides from the tail of the transcript.
pub fn fake_model(messages: &[Message]) -> Vec<(u64, ModelEvent)> {
    use ModelEvent::{Done, TextDelta};
    let tc = |id: &str, name: &str, args: &str| ModelEvent::ToolCall(ToolCall { id: id.into(), name: name.into(), args: args.into() });
    match messages.last() {
        Some(Message::User(t)) if t.contains("weather") => vec![
            (10, TextDelta("Checking ".into())),
            (10, TextDelta("both cities. ".into())),
            (10, tc(&format!("w1-{}", messages.len()), "get_weather", "Paris")),
            (0, tc(&format!("w2-{}", messages.len()), "get_weather", "Oslo")),
            (10, Done(StopReason::ToolUse)),
        ],
        Some(Message::User(t)) if t.contains("slow") => vec![
            (10, tc("s1", "slow_search", "q")),
            (10, Done(StopReason::ToolUse)),
        ],
        Some(Message::User(t)) if t.contains("hang") => vec![
            (10, tc("h1", "hang", "")),
            (10, Done(StopReason::ToolUse)),
        ],
        Some(Message::User(t)) => vec![
            (10, TextDelta("You ".into())),
            (10, TextDelta("said: ".into())),
            (10, TextDelta(t.clone())),
            (10, Done(StopReason::EndTurn)),
        ],
        Some(Message::ToolResult { .. }) => {
            let results: Vec<String> = messages
                .iter()
                .rev()
                .take_while(|m| !matches!(m, Message::Assistant { .. }))
                .filter_map(|m| match m {
                    Message::ToolResult { content, .. } => Some(content.clone()),
                    _ => None,
                })
                .collect();
            vec![
                (10, TextDelta(format!("Results: {:?}", results))),
                (10, Done(StopReason::EndTurn)),
            ]
        }
        _ => vec![(0, Done(StopReason::EndTurn))],
    }
}

/// (latency_ms, result)
pub fn fake_tool(call: &ToolCall) -> (u64, ToolResult) {
    let ok = |s: &str| ToolResult { content: s.into(), is_error: false };
    match (call.name.as_str(), call.args.as_str()) {
        ("get_weather", "Paris") => (120, ok("Paris 18C")),
        ("get_weather", "Oslo") => (60, ok("Oslo 4C")),
        ("slow_search", _) => (400, ok("found it")),
        ("hang", _) => (5_000, ok("never seen")),
        _ => (10, ToolResult { content: "unknown tool".into(), is_error: true }),
    }
}

pub fn apply_cmd(agent: &mut Agent, now: Instant, cmd: Cmd) {
    match cmd {
        Cmd::Prompt(t) => {
            let _ = agent.prompt(now, t);
        }
        Cmd::Steer(t) => agent.steer(now, t),
        Cmd::FollowUp(t) => agent.follow_up(now, t),
        Cmd::Abort => agent.abort(now),
    }
}

/// Feed one host message into the core. Returns true once the script reported done.
pub fn feed(agent: &mut Agent, now: Instant, msg: HostMsg) -> bool {
    match msg {
        HostMsg::Cmd(c) => apply_cmd(agent, now, c),
        HostMsg::Model(id, ev) => agent.handle_model_event(now, id, ev),
        HostMsg::Tool(id, r) => agent.handle_tool_result(now, &id, r),
        HostMsg::ScriptDone => return true,
    }
    false
}

pub fn show(o: &Output) -> Option<String> {
    match o {
        Output::Event(AgentEvent::MessageUpdate { delta }) => Some(format!("  delta {delta:?}")),
        Output::Event(e) => Some(format!("  event {e:?}")),
        _ => None,
    }
}

/// (name, timed user commands as (at_ms, cmd), tool_timeout_ms)
pub fn scenarios() -> Vec<(&'static str, Vec<(u64, Cmd)>, Option<u64>)> {
    vec![
        (
            "A: prompt, parallel tools, steer during tools, follow-up queued",
            vec![
                (0, Cmd::Prompt("what is the weather in Paris and Oslo?")),
                (50, Cmd::Steer("use Fahrenheit")),
                (80, Cmd::FollowUp("now say bye")),
            ],
            None,
        ),
        ("B: abort during a slow tool", vec![(0, Cmd::Prompt("slow search please")), (100, Cmd::Abort)], None),
        ("C: tool timeout (fake or real clock)", vec![(0, Cmd::Prompt("hang")),], Some(300)),
    ]
}
