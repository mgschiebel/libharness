//! Thread-cost test: N concurrent sessions through the model callback against a server that holds every
//! stream open until all N are connected. Samples /proc/self/task for thread names and VmRSS.
use crate::exec::{drive, Exec};
use crate::{or, req, T};
use mock_server::{MockServer, Scenario, Step};
use provider_adapter::*;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Default)]
pub struct Report {
    pub sessions: usize,
    pub ok: usize,
    pub peak_worker_threads: usize,
    pub peak_mock_threads: usize,
    pub peak_other_threads: usize,
    pub baseline_threads: usize,
    pub rss_delta_kb: i64,
    pub wall_ms: u128,
    pub server_peak_active: usize,
    pub other_names_at_peak: String,
    pub errors: Vec<String>,
}
fn census() -> (usize, usize, usize, String) {
    let (mut w, mut m, mut o) = (0, 0, 0);
    let mut names = std::collections::BTreeMap::<String, usize>::new();
    if let Ok(rd) = std::fs::read_dir("/proc/self/task") {
        for e in rd.flatten() {
            let name = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default();
            let name = name.trim();
            if name.starts_with("lh-ureq-worker") || name.starts_with("lh-std-worker") {
                w += 1
            } else if name.starts_with("mock-") {
                m += 1
            } else {
                o += 1;
                *names.entry(name.to_string()).or_default() += 1;
            }
        }
    }
    (w, m, o, names.iter().map(|(k, v)| format!("{k}x{v}")).collect::<Vec<_>>().join(","))
}
fn rss_kb() -> i64 {
    std::fs::read_to_string("/proc/self/status").ok().and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)?.parse().ok())).unwrap_or(0)
}
pub fn run(t: T, exec: Exec, sessions: usize) -> Report {
    let mut body = Vec::new();
    body.extend_from_slice(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello \"},\"finish_reason\":null}]}\n\n");
    let rest = b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"world\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\ndata: [DONE]\n\n".to_vec();
    let srv = Arc::new(MockServer::start(vec![Scenario::sse_steps(vec![Step::Data(body), Step::WaitActive(sessions), Step::Pause(Duration::from_millis(300)), Step::Data(rest)])]));
    let cb = Arc::new(model_callback(Arc::new(or(&srv)), t));
    let stop = Arc::new(AtomicBool::new(false));
    let (base_w, base_m, base_o, _) = census();
    let _ = base_w;
    let rss0 = rss_kb();
    let sampler = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let (mut pw, mut pm, mut po, mut prss, mut pn) = (0, 0, 0, 0, String::new());
            while !stop.load(SeqCst) {
                let (w, m, o, n) = census();
                pw = pw.max(w);
                pm = pm.max(m);
                if o > po {
                    po = o;
                    pn = n;
                }
                prss = prss.max(rss_kb());
                std::thread::sleep(Duration::from_millis(3));
            }
            (pw, pm, po, prss, pn)
        })
    };
    let t0 = Instant::now();
    let cb2 = cb.clone();
    let res = drive(exec, Duration::from_secs(60), async move {
        let futs: Vec<_> = (0..sessions).map(|i| cb2(req(&format!("s{i}")))).collect();
        futures::future::join_all(futs).await
    });
    let wall_ms = t0.elapsed().as_millis();
    stop.store(true, SeqCst);
    let (pw, pm, po, prss, pn) = sampler.join().unwrap();
    let mut r = Report { sessions, wall_ms, peak_worker_threads: pw, peak_mock_threads: pm, peak_other_threads: po, baseline_threads: base_w + base_m + base_o, rss_delta_kb: prss - rss0, server_peak_active: srv.peak_active(), other_names_at_peak: pn, ..Default::default() };
    match res {
        Ok(v) => {
            for x in v {
                match x {
                    Ok(m) if m.text == "hello world" => r.ok += 1,
                    Ok(m) => r.errors.push(format!("text {:?}", m.text)),
                    Err(e) => r.errors.push(e),
                }
            }
        }
        Err(e) => r.errors.push(e),
    }
    r.errors.dedup();
    r
}
