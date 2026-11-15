//! A loopback mock model service: a real TCP listener on 127.0.0.1 that streams an
//! SSE-like body slowly, and notices when the client hangs up. Nothing leaves the machine.
use cb_core::*;
use futures::channel::mpsc;
use futures::StreamExt;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct ServerLog {
    pub chunks_sent: AtomicU64,
    pub client_closed_at: Mutex<Option<Instant>>,
    pub finished: Mutex<bool>,
}
pub struct MockServer {
    pub url: String,
    pub log: Arc<ServerLog>,
}
/// Serves one connection: head, then each `(delay_ms, bytes)` chunk, then `data: [DONE]`.
/// Between chunks it polls the socket for the client's FIN.
pub fn serve(chunks: Vec<(u64, Vec<u8>)>) -> MockServer {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1/chat", l.local_addr().unwrap());
    let log = Arc::new(ServerLog::default());
    let log2 = log.clone();
    std::thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        let mut req = [0u8; 4096];
        let _ = s.read(&mut req);
        s.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n").unwrap();
        s.set_nonblocking(true).unwrap();
        let hung_up = |s: &mut TcpStream| matches!(s.read(&mut [0u8; 1]), Ok(0));
        for (ms, c) in chunks {
            let until = Instant::now() + Duration::from_millis(ms);
            while Instant::now() < until {
                if hung_up(&mut s) {
                    *log2.client_closed_at.lock().unwrap() = Some(Instant::now());
                    return;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            if s.write_all(&c).is_err() {
                *log2.client_closed_at.lock().unwrap() = Some(Instant::now());
                return;
            }
            log2.chunks_sent.fetch_add(1, SeqCst);
        }
        let _ = s.write_all(b"data: [DONE]\n\n");
        *log2.finished.lock().unwrap() = true;
    });
    MockServer { url, log }
}
/// `n` words as `data: {"t":"wN "}` lines; every third chunk is cut mid-line.
pub fn word_chunks(n: usize, gap_ms: u64) -> Vec<(u64, Vec<u8>)> {
    let mut all = Vec::new();
    for i in 0..n {
        all.extend_from_slice(format!("data: {{\"t\":\"w{i} \"}}\n\n").as_bytes());
    }
    let mut out = Vec::new();
    let per = all.len() / n;
    for (i, c) in all.chunks(per.max(1)).enumerate() {
        out.push((if i == 0 { 0 } else { gap_ms }, c.to_vec()));
    }
    out
}

/// The host's body: a reader thread feeds a channel; dropping the body shuts the socket.
pub struct SockBody {
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    sock: TcpStream,
}
impl BodyStream for SockBody {
    fn next<'a>(&'a mut self) -> BoxFutR<'a, Option<Vec<u8>>> {
        Box::pin(async move { self.rx.next().await })
    }
}
impl Drop for SockBody {
    fn drop(&mut self) {
        let _ = self.sock.shutdown(Shutdown::Both);
    }
}
/// A Transport: connect, send, read the head, return the rest as a lent body.
pub fn connect(req: &HttpReq) -> Result<(HttpHead, Box<dyn BodyStream>), String> {
    let addr = req.url.trim_start_matches("http://").split('/').next().unwrap().to_string();
    let mut s = TcpStream::connect(&addr).map_err(|e| e.to_string())?;
    s.write_all(format!("POST / HTTP/1.1\r\nhost: {addr}\r\ncontent-length: {}\r\n\r\n{}", req.body.len(), req.body).as_bytes()).map_err(|e| e.to_string())?;
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if s.read(&mut b).map_err(|e| e.to_string())? == 0 {
            return Err("closed in head".into());
        }
        head.push(b[0]);
    }
    let text = String::from_utf8_lossy(&head).to_string();
    let status: u16 = text.split_whitespace().nth(1).and_then(|x| x.parse().ok()).ok_or("bad status")?;
    let (tx, rx) = mpsc::unbounded();
    let mut r = s.try_clone().unwrap();
    std::thread::spawn(move || {
        let mut buf = [0u8; 64];
        while let Ok(n) = r.read(&mut buf) {
            if n == 0 || tx.unbounded_send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    Ok((HttpHead { status, headers: vec![] }, Box::new(SockBody { rx, sock: s })))
}
