//! Loopback mock of OpenRouter's chat-completions endpoint, on std sockets only.
//! It replays a scripted HTTP response (status, headers, chunked body), records what the client
//! sent, and reports when the client closed the connection (for the cancel case).
//! Fixtures are hand-built from the documented formats; NOT captured from the live API.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Script {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// Each entry is written as one HTTP chunk, flushed, then followed by `delay`.
    pub chunks: Vec<Vec<u8>>,
    pub delay: Duration,
    /// After the last chunk, keep the response open until the client goes away.
    pub hang: bool,
}

impl Script {
    pub fn sse(body: &str, chunk: usize) -> Script {
        Script {
            status: 200,
            headers: vec![("Content-Type".into(), "text/event-stream".into())],
            chunks: body.as_bytes().chunks(chunk.max(1)).map(|c| c.to_vec()).collect(),
            delay: Duration::from_millis(2),
            hang: false,
        }
    }
    pub fn sse_chunks(chunks: &[&str]) -> Script {
        Script {
            status: 200,
            headers: vec![("Content-Type".into(), "text/event-stream".into())],
            chunks: chunks.iter().map(|c| c.as_bytes().to_vec()).collect(),
            delay: Duration::from_millis(2),
            hang: false,
        }
    }
    pub fn http(status: u16, headers: &[(&str, &str)], body: &str) -> Script {
        Script {
            status,
            headers: headers.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
            chunks: vec![body.as_bytes().to_vec()],
            delay: Duration::from_millis(0),
            hang: false,
        }
    }
    pub fn hanging(mut self) -> Script {
        self.hang = true;
        self
    }
}

#[derive(Clone, Debug)]
pub struct Recorded {
    pub request_line: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}
impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("request body is JSON")
    }
}

pub struct Mock {
    pub port: u16,
    pub requests: Arc<Mutex<Vec<Recorded>>>,
    /// Set once the client side of a hanging response is gone (EOF or reset seen by the server).
    pub client_closed: Arc<AtomicBool>,
    /// Number of body chunks written so far across all connections.
    pub chunks_sent: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl Mock {
    pub fn start(script: Script) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let m = Mock {
            port,
            requests: Default::default(),
            client_closed: Default::default(),
            chunks_sent: Default::default(),
            stop: Default::default(),
        };
        let (reqs, closed, sent, stop) = (m.requests.clone(), m.client_closed.clone(), m.chunks_sent.clone(), m.stop.clone());
        thread::spawn(move || {
            for conn in listener.incoming() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(conn) = conn else { continue };
                let (script, reqs, closed, sent) = (script.clone(), reqs.clone(), closed.clone(), sent.clone());
                thread::spawn(move || serve(conn, script, reqs, closed, sent));
            }
        });
        m
    }
    /// Base URL in the form genai expects (trailing slash).
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/api/v1/", self.port)
    }
    pub fn wait_closed(&self, within: Duration) -> bool {
        let end = Instant::now() + within;
        while Instant::now() < end {
            if self.client_closed.load(Ordering::SeqCst) {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        self.client_closed.load(Ordering::SeqCst)
    }
    pub fn wait_chunks(&self, n: usize, within: Duration) -> bool {
        let end = Instant::now() + within;
        while Instant::now() < end {
            if self.chunks_sent.load(Ordering::SeqCst) >= n {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }
    pub fn last_request(&self) -> Recorded {
        self.requests.lock().unwrap().last().cloned().expect("a request reached the mock")
    }
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn serve(mut s: TcpStream, script: Script, reqs: Arc<Mutex<Vec<Recorded>>>, closed: Arc<AtomicBool>, sent: Arc<AtomicUsize>) {
    s.set_nodelay(true).ok();
    // -- read the request head, then the body by Content-Length
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        match s.read(&mut tmp) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("").to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        match s.read(&mut tmp) {
            Ok(0) | Err(_) => break,
            Ok(n) => body.extend_from_slice(&tmp[..n]),
        }
    }
    reqs.lock().unwrap().push(Recorded { request_line, headers, body });

    // -- replay the script
    let mut out = format!("HTTP/1.1 {} X\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n", script.status);
    for (k, v) in &script.headers {
        out += &format!("{k}: {v}\r\n");
    }
    out += "\r\n";
    if s.write_all(out.as_bytes()).is_err() {
        closed.store(true, Ordering::SeqCst);
        return;
    }
    for c in &script.chunks {
        let ok = s.write_all(format!("{:x}\r\n", c.len()).as_bytes()).is_ok() && s.write_all(c).is_ok() && s.write_all(b"\r\n").is_ok() && s.flush().is_ok();
        if !ok {
            closed.store(true, Ordering::SeqCst);
            return;
        }
        sent.fetch_add(1, Ordering::SeqCst);
        thread::sleep(script.delay);
    }
    if script.hang {
        // Wait for the client to disappear: EOF or a hard error on read means the socket is gone.
        s.set_read_timeout(Some(Duration::from_millis(20))).ok();
        let end = Instant::now() + Duration::from_secs(20);
        while Instant::now() < end {
            match s.read(&mut tmp) {
                Ok(0) => break,
                Ok(_) => continue,
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
                Err(_) => break,
            }
        }
        if Instant::now() < end {
            closed.store(true, Ordering::SeqCst);
        }
        return;
    }
    let _ = s.write_all(b"0\r\n\r\n");
    let _ = s.flush();
}
