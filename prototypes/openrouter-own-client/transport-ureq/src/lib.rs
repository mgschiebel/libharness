//! `ureq` 3 (blocking) driven from one worker thread per call.
use provider_adapter::bridge::{spawn_call, Item, Sender};
use provider_adapter::provider_codec::{HttpRequest, ResponseHead};
use provider_adapter::{Body, BoxFut, Transport};
use std::io::Read;

#[derive(Clone)]
pub struct UreqTransport {
    cfg: ureq::config::Config,
    /// `Some`: one shared agent (pooling) and NO way to interrupt a blocked read: the naive build.
    shared: Option<ureq::Agent>,
}
impl UreqTransport {
    /// One `Agent` per call, so that `Agent::shutdown()` on drop interrupts only that call's socket.
    /// ureq 3 has no per-request cancel; `shutdown` is agent-wide, hence the agent per call (no pooling).
    pub fn new() -> Self {
        Self { cfg: Self::config(), shared: None }
    }
    /// The first attempt: one shared agent, cancel = set a flag the worker only sees after the next chunk.
    pub fn naive() -> Self {
        let cfg = Self::config();
        Self { shared: Some(ureq::Agent::new_with_config(cfg.clone())), cfg }
    }
    fn config() -> ureq::config::Config {
        // 4xx/5xx must come back as responses (OpenRouter's JSON error bodies), not as `Err`.
        ureq::Agent::config_builder().http_status_as_error(false).proxy(None).build()
    }
}
impl Transport for UreqTransport {
    fn send(&self, req: HttpRequest) -> BoxFut<'static, Result<(ResponseHead, Box<dyn Body>), String>> {
        let (cfg, shared) = (self.cfg.clone(), self.shared.clone());
        spawn_call("lh-ureq-worker", move |tx| {
            let agent = shared.unwrap_or_else(|| {
                let agent = ureq::Agent::new_with_config(cfg);
                let a = agent.clone();
                tx.on_cancel(move || a.shutdown());
                agent
            });
            if let Err(e) = run(&agent, &req, tx) {
                tx.send(Item::Fail(e));
            }
        })
    }
}

fn run(agent: &ureq::Agent, req: &HttpRequest, tx: &Sender) -> Result<(), String> {
    let mut b = agent.post(&req.url);
    for (k, v) in &req.headers {
        b = b.header(k.as_str(), v.as_str());
    }
    let resp = b.send(&req.body[..]).map_err(|e| e.to_string())?;
    let headers = resp.headers().iter().map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect();
    if !tx.send(Item::Head(ResponseHead { status: resp.status().as_u16(), headers })) {
        return Ok(());
    }
    let mut rd = resp.into_body().into_reader();
    let mut buf = [0u8; 8192];
    loop {
        let n = rd.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 || !tx.send(Item::Chunk(buf[..n].to_vec())) {
            return Ok(());
        }
    }
}
