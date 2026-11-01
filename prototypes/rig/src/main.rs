//! PROTOTYPE (throwaway): rig 0.46 against libharness requirements. See README.md.
use std::sync::{Arc, Mutex};
use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
use rig_agent::{AgentBuilder, tool::{Tool, ToolContext}};
use rig_core::providers::openai::{wire::{OpenAIConfig, OPENROUTER}};
use rig_memory::DirStore;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::{BTreeSet, VecDeque}, sync::Arc as A};
use rig_agent::run::{AgentRun, AgentRunStep, ModelTurn, PendingToolCall, TurnPolicy};
use rig_core::{completion::{CompletionRequest, Message}, tool::{ToolOutput, ToolResult}};
use rig_core::completion::ToolDefinition;

#[derive(Default)]
struct Mock { script: Vec<Value>, seen: Vec<(String, Value)> }
type Shared = Arc<Mutex<Mock>>;

async fn chat(State(s): State<Shared>, h: HeaderMap, Json(body): Json<Value>) -> Json<Value> {
    let mut m = s.lock().unwrap();
    let auth = h.get("authorization").map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
    m.seen.push((auth, body));
    let r = if m.script.is_empty() { text("(script exhausted)") } else { m.script.remove(0) };
    Json(r)
}
fn text(t: &str) -> Value {
    json!({"id":"x","object":"chat.completion","created":0,"model":"m","choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":t}}],"usage":{"prompt_tokens":5,"completion_tokens":5,"total_tokens":10}})
}
fn tool_call(id: &str, name: &str, args: &str) -> Value {
    json!({"id":"x","object":"chat.completion","created":0,"model":"m","choices":[{"index":0,"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":args}}]}}],"usage":{"prompt_tokens":5,"completion_tokens":5,"total_tokens":10}})
}

#[derive(Deserialize)] struct AddArgs { a: i64, b: i64 }
#[derive(Debug)] struct AddErr;
impl std::fmt::Display for AddErr { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "add err") } }
impl std::error::Error for AddErr {}
/// Host-owned side effect: the tool just counts invocations in host state.
struct Add(Arc<Mutex<u32>>);
impl Tool for Add {
    const NAME: &'static str = "add";
    type Args = AddArgs; type Output = i64; type Error = AddErr;
    fn description(&self) -> String { "add two ints".into() }
    fn parameters(&self) -> Value { json!({"type":"object","properties":{"a":{"type":"integer"},"b":{"type":"integer"}},"required":["a","b"]}) }
    async fn call(&self, _c: &mut ToolContext, a: AddArgs) -> Result<i64, AddErr> { *self.0.lock().unwrap() += 1; Ok(a.a + a.b) }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let shared: Shared = Default::default();
    let app = Router::new().route("/api/v1/chat/completions", post(chat)).with_state(shared.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}/api/v1", l.local_addr()?);
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });

    // 1+2: prompt + tool call, custom base URL (OpenRouter dialect)
    shared.lock().unwrap().script = vec![tool_call("c1", "add", r#"{"a":2,"b":3}"#), text("The sum is 5.")];
    let cfg = OpenAIConfig::with_key(&OPENROUTER, "sk-or-test").with_base_url(base.clone());
    let model = cfg.client().chat("openai/gpt-x");
    let calls = Arc::new(Mutex::new(0));
    let dir = std::env::temp_dir().join("proto-rig-sessions");
    let _ = std::fs::remove_dir_all(&dir);
    let agent = AgentBuilder::new(model.clone())
        .preamble("be brief")
        .tool(Add(calls.clone()))
        .memory(DirStore::open(&dir)?)
        .conversation("s1")
        .build();
    let out = agent.prompt("what is 2+3?").max_turns(4).await?;
    println!("[1/2] prompt+tool: answer={:?} tool_calls_run={} usage_total={:?}", out.content, calls.lock().unwrap(), out.usage.total_tokens);
    for (auth, body) in &shared.lock().unwrap().seen { println!("  req auth={auth} model={} msgs={} tools={}", body["model"], body["messages"].as_array().unwrap().len(), body["tools"].as_array().map_or(0, |t| t.len())); }
    println!("  files: {:?}", std::fs::read_dir(&dir)?.map(|e| e.unwrap().path()).collect::<Vec<_>>());
    // 3: restart persistence (new agent value, same store/log)
    shared.lock().unwrap().script = vec![text("You asked 2+3.")];
    let agent2 = AgentBuilder::new(model.clone()).memory(DirStore::open(&dir)?).conversation("s1").build();
    let out = agent2.prompt("what did I ask?").await?;
    let seen = shared.lock().unwrap().seen.last().unwrap().1["messages"].as_array().unwrap().len();
    println!("[3] persistence: reply={:?}; model was sent {seen} msgs after restart (expect 5 = 4 kept + new prompt)", out.content);

    // 4: compaction. Sans-IO plan -> host sends summary request -> apply.
    {
        use rig_memory::{load, save, CompactionPolicy, CompactReason, SummaryState, apply_summary, summarized_prefix, Summarizer, ClearToolOutputs, summary_author};
        let store = DirStore::open(&dir)?;
        let mut conv = load(&store, "s1")?;
        println!("[4] loaded {} msgs", conv.messages().len());
        let mut spec = rig_core::catalog::ModelSpec::new(rig_core::providers::registry::ProviderId::catalog("openrouter").expect("pid"), "openai/gpt-x");
        spec.context_window = Some(2000);
        let policy = CompactionPolicy::default();
        match policy.plan_summary(&conv, &SummaryState::default(), &CompactReason::Asked { focus: "numbers".into() }, &spec)? {
            None => println!("  plan_summary: nothing to summarise (keep_asked window covers it)"),
            Some(plan) => {
                println!("  plan: summary request built, keep_from={:?}", plan.keep_from);
                shared.lock().unwrap().script = vec![text("SUMMARY: user asked 2+3, answer 5.")];
                let resp = model.call(plan.request).await?;
                let sum = Summarizer::summary_text(&resp)?;
                let mut st = plan.state; st.summary = sum;
                apply_summary(&mut conv, plan.keep_from, &st);
                save(&store, "s1", &mut conv)?;
                println!("  applied; now {} msgs; first={:?}", conv.messages().len(), conv.messages()[0]);
            }
        }
        let c = ClearToolOutputs::new(0).clear_conversation(&mut conv, summary_author());
        println!("  ClearToolOutputs (no model call): {} results cleared", c.results);
        let _ = (summarized_prefix(&conv),);
    }

    // 5: host-driven stepper with steering: sans-IO AgentRun, host owns model call + tools + queues.
    shared.lock().unwrap().script = vec![tool_call("c9", "add", r#"{"a":1,"b":1}"#), text("Steered: switched to multiplication."), ];
    let mut steer: VecDeque<String> = VecDeque::new();
    let defs = vec![A::new(ToolDefinition::new(rig_core::message::ToolName::new("add")?, "add two ints", json!({"type":"object","properties":{"a":{"type":"integer"},"b":{"type":"integer"}},"required":["a","b"]})))];
    let policy = TurnPolicy::new(BTreeSet::from(["add".to_string()]), None, None)?;
    let mut run = AgentRun::new(Message::user("add 1+1")).max_turns(8);
    let mut log = vec![];
    loop {
        match run.next_step()? {
            AgentRunStep::CallModel { prompt, history, turn } => {
                if turn > 1 {
                    if let Some(s) = steer.pop_front() {
                        // no native inject API: restart run from canonical history with the steering message as prompt
                        let hist = run.full_history();
                        log.push(format!("steer@turn{turn}: restart run with {} msgs of history", hist.len()));
                        run = AgentRun::new(Message::user(s)).with_history(hist).max_turns(8);
                        continue;
                    }
                }
                let mut req = CompletionRequest::new((*prompt).clone()).tools(defs.clone());
                let mut h = history; h.push(prompt.clone()); req.chat_history = h;
                log.push(format!("model call turn {turn}"));
                let resp = model.call(req).await?;
                run.model_response(ModelTurn::from_policy(&resp, policy.clone()))?;
            }
            AgentRunStep::CallTools { calls } => {
                steer.push_back("actually multiply instead".into()); // host's UI enqueues steering mid-run
                let mut answers = vec![];
                for c in calls { if let PendingToolCall::Execute(e) = c {
                    let a: AddArgs = serde_json::from_value(e.arguments())?;
                    log.push(format!("host runs tool {} -> {}", e.name(), a.a + a.b));
                    answers.push(e.answer(ToolResult::success(ToolOutput::text((a.a + a.b).to_string()))));
                } }
                run.answer_all(answers)?;
            }
            AgentRunStep::Done(r) => { log.push(format!("done: {:?}", r.content)); break; }
        }
    }
    println!("[5] stepper+steering:\n  {}", log.join("\n  "));
    let last = shared.lock().unwrap().seen.last().unwrap().1["messages"].clone();
    println!("  last request roles: {:?}", last.as_array().unwrap().iter().map(|m| m["role"].as_str().unwrap().to_string()).collect::<Vec<_>>());
    // 6: OpenRouter extras via additional_params
    shared.lock().unwrap().script = vec![text("ok")];
    let a3 = AgentBuilder::new(model.clone()).additional_params(json!({"provider":{"order":["anthropic"]},"models":["a/b","c/d"],"reasoning":{"effort":"low"}})).build();
    a3.prompt("hi").await?;
    let b = shared.lock().unwrap().seen.last().unwrap().1.clone();
    println!("[6] extras on wire: provider={} models={} reasoning={}", b["provider"], b["models"], b["reasoning"]);
    Ok(())
}
