//! Loopback mock HTTP/1.1 server on std sockets. One thread per connection (named `mock-conn`).
//! Replays a `Scenario`: status + headers + either a chunked body of scripted steps or a fixed
//! Content-Length body. Records requests and whether the client closed the socket early.
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub enum Step {
    /// One HTTP chunk, flushed immediately.
    Data(Vec<u8>),
    Pause(Duration),
    /// Keep the connection open until the client closes it (or 30 s). Sets `client_closed`.
    Hold,
    /// Wait until `n` connections are in flight at once (or 30 s), then go on.
    WaitActive(usize),
    /// Close the socket without the terminating `0` chunk.
    Abort,
}
pub enum BodyPlan {
    Chunked(Vec<Step>),
    Fixed(Vec<u8>),
}
pub struct Scenario {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: BodyPlan,
}
impl Scenario {
    /// `200 text/event-stream`, the bytes cut into HTTP chunks of `piece` bytes (not at line edges).
    pub fn sse(bytes: &[u8], piece: usize) -> Self {
        Self::sse_steps(bytes.chunks(piece.max(1)).map(|c| Step::Data(c.to_vec())).collect())
    }
    pub fn sse_steps(steps: Vec<Step>) -> Self {
        Scenario { status: 200, headers: vec![("Content-Type".into(), "text/event-stream".into())], body: BodyPlan::Chunked(steps) }
    }
    pub fn fixed(status: u16, headers: &[(&str, &str)], body: &str) -> Self {
        let mut h: Vec<(String, String)> = headers.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        h.push(("Content-Type".into(), "application/json".into()));
        Scenario { status, headers: h, body: BodyPlan::Fixed(body.as_bytes().to_vec()) }
    }
}
#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}
impl Recorded {
    pub fn header(&self, n: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.as_str())
    }
    pub fn json(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}
#[derive(Default)]
struct Conn {
    client_closed: AtomicBool,
    done: AtomicBool,
}
struct Shared {
    scenarios: Vec<Scenario>,
    next: AtomicUsize,
    requests: Mutex<Vec<Recorded>>,
    conns: Mutex<Vec<Arc<Conn>>>,
    active: AtomicUsize,
    peak: AtomicUsize,
}
pub struct MockServer {
    pub addr: std::net::SocketAddr,
    sh: Arc<Shared>,
}
impl MockServer {
    /// Connection `n` gets `scenarios[n]`; past the end, the last one repeats.
    pub fn start(scenarios: Vec<Scenario>) -> Self {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let sh = Arc::new(Shared { scenarios, next: AtomicUsize::new(0), requests: Default::default(), conns: Default::default(), active: AtomicUsize::new(0), peak: AtomicUsize::new(0) });
        let sh2 = sh.clone();
        std::thread::Builder::new()
            .name("mock-accept".into())
            .spawn(move || {
                for s in l.incoming().flatten() {
                    let idx = sh2.next.fetch_add(1, SeqCst);
                    let conn = Arc::new(Conn::default());
                    sh2.conns.lock().unwrap().push(conn.clone());
                    let sh3 = sh2.clone();
                    std::thread::Builder::new().name("mock-conn".into()).spawn(move || handle(s, &sh3, idx, &conn)).ok();
                }
            })
            .unwrap();
        MockServer { addr, sh }
    }
    pub fn base_url(&self) -> String {
        format!("http://{}/api/v1", self.addr)
    }
    pub fn requests(&self) -> Vec<Recorded> {
        self.sh.requests.lock().unwrap().clone()
    }
    pub fn connections(&self) -> usize {
        self.sh.next.load(SeqCst)
    }
    pub fn peak_active(&self) -> usize {
        self.sh.peak.load(SeqCst)
    }
    /// Non-blocking: has the client closed connection `idx` while the server still had a stream open?
    pub fn client_closed(&self, idx: usize) -> bool {
        self.sh.conns.lock().unwrap().get(idx).map_or(false, |c| c.client_closed.load(SeqCst))
    }
    /// Blocking variant of `client_closed`.
    pub fn wait_client_closed(&self, idx: usize, within: Duration) -> bool {
        let end = Instant::now() + within;
        while Instant::now() < end {
            if let Some(c) = self.sh.conns.lock().unwrap().get(idx) {
                if c.client_closed.load(SeqCst) {
                    return true;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }
    pub fn wait_done(&self, idx: usize, within: Duration) -> bool {
        let end = Instant::now() + within;
        while Instant::now() < end {
            if self.sh.conns.lock().unwrap().get(idx).map_or(false, |c| c.done.load(SeqCst)) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }
}

fn read_request(s: &mut TcpStream) -> Option<Recorded> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        let n = s.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let (method, path) = (first.next()?.to_string(), first.next()?.to_string());
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    let len: usize = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let mut body = buf[head_end..].to_vec();
    while body.len() < len {
        let n = s.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    Some(Recorded { method, path, headers, body })
}

fn reason(c: u16) -> &'static str {
    match c {
        200 => "OK",
        402 => "Payment Required",
        429 => "Too Many Requests",
        503 => "Service Unavailable",
        _ => "Status",
    }
}
fn handle(mut s: TcpStream, sh: &Shared, idx: usize, conn: &Conn) {
    s.set_nodelay(true).ok();
    let Some(rec) = read_request(&mut s) else { return };
    sh.requests.lock().unwrap().push(rec);
    let now = sh.active.fetch_add(1, SeqCst) + 1;
    sh.peak.fetch_max(now, SeqCst);
    let sc = &sh.scenarios[idx.min(sh.scenarios.len() - 1)];
    let mut head = format!("HTTP/1.1 {} {}\r\nConnection: close\r\n", sc.status, reason(sc.status));
    for (k, v) in &sc.headers {
        head += &format!("{k}: {v}\r\n");
    }
    match &sc.body {
        BodyPlan::Fixed(b) => {
            head += &format!("Content-Length: {}\r\n\r\n", b.len());
            let mut out = head.into_bytes();
            out.extend_from_slice(b);
            let _ = s.write_all(&out);
        }
        BodyPlan::Chunked(steps) => {
            head += "Transfer-Encoding: chunked\r\n\r\n";
            let mut ok = s.write_all(head.as_bytes()).is_ok();
            let mut clean = true;
            for st in steps {
                if !ok {
                    break;
                }
                match st {
                    Step::Data(d) => {
                        let mut out = format!("{:x}\r\n", d.len()).into_bytes();
                        out.extend_from_slice(d);
                        out.extend_from_slice(b"\r\n");
                        ok = s.write_all(&out).is_ok();
                    }
                    Step::Pause(d) => std::thread::sleep(*d),
                    Step::WaitActive(n) => {
                        let end = Instant::now() + Duration::from_secs(30);
                        while sh.peak.load(SeqCst) < *n && Instant::now() < end {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    }
                    Step::Hold => {
                        s.set_read_timeout(Some(Duration::from_millis(10))).ok();
                        let end = Instant::now() + Duration::from_secs(30);
                        let mut b = [0u8; 64];
                        while Instant::now() < end {
                            match s.read(&mut b) {
                                Ok(0) => {
                                    ok = false;
                                    break;
                                }
                                Ok(_) => {}
                                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                                Err(_) => {
                                    ok = false;
                                    break;
                                }
                            }
                        }
                    }
                    Step::Abort => {
                        clean = false;
                        break;
                    }
                }
            }
            if !ok {
                conn.client_closed.store(true, SeqCst);
            } else if clean {
                let _ = s.write_all(b"0\r\n\r\n");
            }
        }
    }
    let _ = s.shutdown(Shutdown::Both);
    sh.active.fetch_sub(1, SeqCst);
    conn.done.store(true, SeqCst);
}
