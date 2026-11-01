//! PROTOTYPE (throwaway, #27): std host glue around the unchanged no_std core.
use nostd_core::{demo::Answer, Capabilities, CoreError, ModelReply, ToolCall};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{atomic::{AtomicU64, Ordering::SeqCst}, Arc};
use std::time::Instant;

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("core: {0}")]
    Core(#[from] CoreError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Serialize, Deserialize, schemars::JsonSchema)]
pub struct Path { pub path: String }
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
pub struct Write_ { pub path: String, pub text: String }
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
pub struct Url { pub url: String }

#[derive(Default)]
pub struct Probe { pub files_written: AtomicU64, pub bytes_read: AtomicU64, pub http_ok: AtomicU64 }

/// Local HTTP stub: std TcpListener, thread per connection, answers `ok:<path>`.
pub struct Stub { pub addr: SocketAddr, pub hits: Arc<AtomicU64> }
pub fn spawn_stub() -> Stub {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let hits = Arc::new(AtomicU64::new(0));
    let h = hits.clone();
    std::thread::spawn(move || for s in l.incoming().flatten() {
        let h = h.clone();
        std::thread::spawn(move || { let mut s = s; let mut b = [0u8; 512]; let mut n = 0; while n < 512 && !b[..n].windows(4).any(|w| w == b"\r\n\r\n") { match s.read(&mut b[n..]) { Ok(0) | Err(_) => break, Ok(k) => n += k } }
            let line = String::from_utf8_lossy(&b[..n]); let p = line.split_whitespace().nth(1).unwrap_or("/").to_string();
            let body = format!("ok:{p}"); h.fetch_add(1, SeqCst);
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()); });
    });
    Stub { addr, hits }
}
fn http_get(addr: &str, path: &str) -> std::io::Result<String> {
    let mut s = TcpStream::connect(addr)?;
    write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")?;
    let mut r = String::new(); s.read_to_string(&mut r)?;
    Ok(r.rsplit("\r\n\r\n").next().unwrap_or("").to_string())
}

/// Real-I/O capabilities: callbacks do blocking std fs + TCP inside the async block
/// (executor-agnostic; the same futures run on tokio and futures-executor).
pub fn io_caps(dir: std::path::PathBuf, stub: SocketAddr, probe: Arc<Probe>, tag: u64) -> Capabilities<Answer> {
    let t0 = Instant::now();
    let (p1, p2, p3) = (probe.clone(), probe.clone(), probe);
    let (d1, d2) = (dir.clone(), dir);
    Capabilities::<Answer>::new()
        .clock(move || t0.elapsed().as_millis() as u64) // injected Instant
        .tool_typed("write_file", "write temp file", move |a: Write_| { let (p, d) = (p1.clone(), d1.clone()); async move {
            std::fs::write(d.join(&a.path), &a.text).map_err(|e| e.to_string())?; p.files_written.fetch_add(1, SeqCst); Ok(a.text.len()) } })
        .tool_typed("read_file", "read temp file", move |a: Path| { let (p, d) = (p2.clone(), d2.clone()); async move {
            let s = std::fs::read_to_string(d.join(&a.path)).map_err(|e| e.to_string())?; p.bytes_read.fetch_add(s.len() as u64, SeqCst); Ok(s) } })
        .tool_typed("http_get", "GET local stub", move |a: Url| { let (p, addr) = (p3.clone(), stub.to_string()); async move {
            let b = http_get(&addr, &a.url).map_err(|e| { eprintln!("http err {e}"); e.to_string() })?; if !b.starts_with("ok:") { eprintln!("bad body {b:?}"); } if b.starts_with("ok:") { p.http_ok.fetch_add(1, SeqCst); } Ok(b) } })
        .model(move |_| async move {
            let c = |id: &str, n: &str, a: serde_json::Value| ToolCall { id: id.into(), name: n.into(), args: a };
            let f = format!("s{tag}.txt");
            Ok(ModelReply { text: String::new(), tool_calls: vec![
                c("1", "write_file", serde_json::json!({"path": f, "text": format!("session {tag}")})),
                c("2", "read_file", serde_json::json!({"path": f})),
                c("3", "http_get", serde_json::json!({"url": format!("/s{tag}")})),
                c("4", "final_answer", serde_json::json!({"total": tag, "note": "io"})),
            ]})
        })
}

/// `?` across the boundary: CoreError -> HostError -> Box<dyn std::error::Error>.
pub async fn run_session(caps: &Capabilities<Answer>) -> Result<Answer, HostError> {
    let a = caps.run_once("go").await?;
    Ok(a)
}
pub fn boxed(caps_err: CoreError) -> Result<(), Box<dyn std::error::Error + Send + Sync>> { Err(caps_err)? }

/// Pure-CPU contention scenario: K no-op tool calls per session, optional yield (tokio-free).
pub fn cpu_caps(k: usize) -> Capabilities<Answer> {
    Capabilities::<Answer>::new()
        .tool_typed("nop", "nop", |a: Path| async move { YieldOnce(false).await; Ok(a.path.len()) })
        .model(move |_| async move {
            let mut calls: Vec<ToolCall> = (0..k).map(|i| ToolCall { id: i.to_string(), name: "nop".into(), args: serde_json::json!({"path": "x"}) }).collect();
            calls.push(ToolCall { id: "f".into(), name: "final_answer".into(), args: serde_json::json!({"total": k as i64, "note": "cpu"}) });
            Ok(ModelReply { text: String::new(), tool_calls: calls })
        })
}
/// Executor-agnostic yield point so tasks interleave on any executor.
pub struct YieldOnce(pub bool);
impl std::future::Future for YieldOnce {
    type Output = ();
    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        if self.0 { std::task::Poll::Ready(()) } else { self.0 = true; cx.waker().wake_by_ref(); std::task::Poll::Pending }
    }
}
