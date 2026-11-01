//! Sync host (std threads + mpsc). Compare line count with host_async.rs.
use sans_io_host_proto::toy::*;
use sans_io_host_proto::*;
use std::sync::mpsc;
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

fn main() {
    for (name, script, tmo) in scenarios() {
        println!("\n== {name}");
        let (tx, rx) = mpsc::channel::<HostMsg>();
        let utx = tx.clone();
        thread::spawn(move || {
            let mut t = 0;
            for (at, c) in script {
                sleep(Duration::from_millis(at - t));
                t = at;
                utx.send(HostMsg::Cmd(c)).unwrap();
            }
            utx.send(HostMsg::ScriptDone).unwrap();
        });
        let mut agent = Agent::new(Config { tool_timeout: tmo.map(Duration::from_millis) });
        let mut script_done = false;
        loop {
            while let Some(out) = agent.poll_output() {
                if let Some(s) = show(&out) {
                    println!("{s}");
                }
                match out {
                    Output::CallModel(req) => {
                        let tx = tx.clone();
                        thread::spawn(move || {
                            for (ms, ev) in fake_model(&req.messages) {
                                sleep(Duration::from_millis(ms));
                                let _ = tx.send(HostMsg::Model(req.call_id, ev));
                            }
                        });
                    }
                    Output::RunTool(call) => {
                        let tx = tx.clone();
                        thread::spawn(move || {
                            let (ms, res) = fake_tool(&call);
                            sleep(Duration::from_millis(ms));
                            let _ = tx.send(HostMsg::Tool(call.id, res));
                        });
                    }
                    // std threads cannot be cancelled; the core ignores stale results.
                    Output::CancelModel { .. } | Output::CancelTool { .. } | Output::Event(_) => {}
                }
            }
            if script_done && agent.is_idle() {
                break;
            }
            let msg = match agent.poll_timeout() {
                Some(d) => rx.recv_timeout(d.saturating_duration_since(Instant::now())).ok(),
                None => rx.recv().ok(),
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
