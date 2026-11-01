//! Async host (tokio). Same shape as host_sync.rs; the diff is the point.
use sans_io_host_proto::toy::*;
use sans_io_host_proto::*;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio::time::{sleep, sleep_until};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    for (name, script, tmo) in scenarios() {
        println!("\n== {name}");
        let (tx, mut rx) = mpsc::unbounded_channel::<HostMsg>();
        let utx = tx.clone();
        tokio::spawn(async move {
            let mut t = 0;
            for (at, c) in script {
                sleep(Duration::from_millis(at - t)).await;
                t = at;
                utx.send(HostMsg::Cmd(c)).unwrap();
            }
            utx.send(HostMsg::ScriptDone).unwrap();
        });
        let mut agent = Agent::new(Config { tool_timeout: tmo.map(Duration::from_millis) });
        let mut script_done = false;
        let mut tasks: HashMap<String, AbortHandle> = HashMap::new(); // real cancellation
        loop {
            while let Some(out) = agent.poll_output() {
                if let Some(s) = show(&out) {
                    println!("{s}");
                }
                match out {
                    Output::CallModel(req) => {
                        let tx = tx.clone();
                        let h = tokio::spawn(async move {
                            for (ms, ev) in fake_model(&req.messages) {
                                sleep(Duration::from_millis(ms)).await;
                                let _ = tx.send(HostMsg::Model(req.call_id, ev));
                            }
                        });
                        tasks.insert(format!("model-{}", req.call_id), h.abort_handle());
                    }
                    Output::RunTool(call) => {
                        let tx = tx.clone();
                        let id = call.id.clone();
                        let h = tokio::spawn(async move {
                            let (ms, res) = fake_tool(&call);
                            sleep(Duration::from_millis(ms)).await;
                            let _ = tx.send(HostMsg::Tool(call.id, res));
                        });
                        tasks.insert(id, h.abort_handle());
                    }
                    Output::CancelModel { call_id } => {
                        if let Some(h) = tasks.remove(&format!("model-{call_id}")) {
                            h.abort();
                        }
                    }
                    Output::CancelTool { tool_call_id } => {
                        if let Some(h) = tasks.remove(&tool_call_id) {
                            h.abort();
                        }
                    }
                    Output::Event(_) => {}
                }
            }
            if script_done && agent.is_idle() {
                break;
            }
            let deadline = agent.poll_timeout();
            let msg = tokio::select! {
                m = rx.recv() => m,
                _ = async { match deadline {
                        Some(d) => sleep_until(d.into()).await,
                        None => std::future::pending().await,
                    } } => None,
            };
            let now = Instant::now();
            match msg {
                Some(m) => script_done |= feed(&mut agent, now, m),
                None => agent.handle_timeout(now),
            }
        }
        println!("  final history: {} messages", agent.messages().len());
    }
}
