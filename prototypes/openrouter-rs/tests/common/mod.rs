//! Loopback mock OpenRouter server on std sockets, plus hand-built SSE fixtures.
//! Fixtures are built from the documented formats and the OpenAPI schemas (ChatStreamChunk,
//! ChatStreamToolCall, ChatUsage); they are NOT captured from the live API.
#![allow(dead_code)]
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    /// `Some(n)`: stream as `Transfer-Encoding: chunked` in HTTP chunks of `n` bytes (may split UTF-8).
    /// `None`: plain `Content-Length` body.
    pub chunk: Option<usize>,
    /// After the body, keep the connection open (the cancel test).
    pub hold: bool,
    /// Close the connection without the chunked terminator (truncated stream).
    pub abrupt: bool,
}
impl Reply {
    pub fn sse(body: impl Into<String>, chunk: usize) -> Self {
        Reply {
            status: 200,
            headers: vec![],
            body: body.into(),
            chunk: Some(chunk),
            hold: false,
            abrupt: false,
        }
    }
    pub fn http(status: u16, headers: &[(&str, &str)], body: &str) -> Self {
        Reply {
            status,
            headers: headers
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
            body: body.into(),
            chunk: None,
            hold: false,
            abrupt: false,
        }
    }
    pub fn hold(mut self) -> Self {
        self.hold = true;
        self
    }
    pub fn abrupt(mut self) -> Self {
        self.abrupt = true;
        self
    }
}

#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}
impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap()
    }
}

pub struct Mock {
    pub port: u16,
    pub requests: Arc<Mutex<Vec<Recorded>>>,
    pub accepted: Arc<AtomicUsize>,
    /// Connections whose peer closed while the mock was holding them open.
    pub peer_closed: Arc<AtomicUsize>,
}
impl Mock {
    /// Every connection gets the next reply (the last one repeats).
    pub fn start(replies: Vec<Reply>) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let m = Mock {
            port,
            requests: Default::default(),
            accepted: Default::default(),
            peer_closed: Default::default(),
        };
        let (reqs, acc, closed) = (
            m.requests.clone(),
            m.accepted.clone(),
            m.peer_closed.clone(),
        );
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { break };
                let n = acc.fetch_add(1, Ordering::SeqCst);
                let reply = replies[n.min(replies.len() - 1)].clone();
                let (reqs, closed) = (reqs.clone(), closed.clone());
                std::thread::spawn(move || serve(conn, reply, reqs, closed));
            }
        });
        m
    }
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/api/v1", self.port)
    }
    pub fn wait_closed(&self, within: Duration) -> bool {
        let t = Instant::now();
        while t.elapsed() < within {
            if self.peer_closed.load(Ordering::SeqCst) > 0 {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }
    pub fn wait_accepted(&self, within: Duration) -> bool {
        let t = Instant::now();
        while t.elapsed() < within {
            if self.accepted.load(Ordering::SeqCst) > 0 && !self.requests.lock().unwrap().is_empty()
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }
}

fn read_request(conn: &mut TcpStream) -> Option<Recorded> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        let n = conn.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let mut first = lines.next()?.split_whitespace();
    let (method, path) = (first.next()?.to_string(), first.next()?.to_string());
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string()))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = conn.read(&mut tmp).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    Some(Recorded {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).to_string(),
    })
}

fn serve(mut conn: TcpStream, r: Reply, reqs: Arc<Mutex<Vec<Recorded>>>, closed: Arc<AtomicUsize>) {
    conn.set_nodelay(true).ok();
    let Some(req) = read_request(&mut conn) else {
        return;
    };
    reqs.lock().unwrap().push(req);
    let mut head = format!("HTTP/1.1 {} X\r\nConnection: close\r\n", r.status);
    for (k, v) in &r.headers {
        head += &format!("{k}: {v}\r\n");
    }
    match r.chunk {
        Some(n) => {
            head += "Content-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
            conn.write_all(head.as_bytes()).ok();
            for piece in r.body.as_bytes().chunks(n.max(1)) {
                let mut frame = format!("{:x}\r\n", piece.len()).into_bytes();
                frame.extend_from_slice(piece);
                frame.extend_from_slice(b"\r\n");
                if conn.write_all(&frame).is_err() {
                    return;
                }
                conn.flush().ok();
                if n < 16 {
                    std::thread::sleep(Duration::from_micros(300));
                }
            }
            if !r.hold && !r.abrupt {
                conn.write_all(b"0\r\n\r\n").ok();
            }
        }
        None => {
            head += &format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                r.body.len(),
                r.body
            );
            conn.write_all(head.as_bytes()).ok();
        }
    }
    if r.hold {
        // Wait for the client to hang up: a read of 0 bytes or an error means the socket was closed.
        conn.set_read_timeout(Some(Duration::from_millis(50))).ok();
        let t = Instant::now();
        let mut b = [0u8; 64];
        while t.elapsed() < Duration::from_secs(20) {
            match conn.read(&mut b) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => break,
            }
        }
        if t.elapsed() < Duration::from_secs(20) {
            closed.fetch_add(1, Ordering::SeqCst);
        }
    }
}

// ---- fixtures ------------------------------------------------------------------------------
pub const REQUESTED: &str = "openai/gpt-4o";
pub const SERVED: &str = "anthropic/claude-3.5-sonnet";

/// One chat.completion.chunk with the envelope fields the crate's chunk type requires.
pub fn chunk(delta: Value, finish: Value, extra: Value) -> Value {
    let mut v = json!({"id":"gen-1","object":"chat.completion.chunk","created":1760000000u64,"model":SERVED,
        "choices":[{"index":0,"delta":delta,"finish_reason":finish}]});
    if let Value::Object(m) = extra {
        for (k, x) in m {
            v[k] = x;
        }
    }
    v
}
pub fn data(v: &Value) -> String {
    format!("data: {v}\n\n")
}
pub const COMMENT: &str = ": OPENROUTER PROCESSING\n\n";

/// Cases 1, 4, 5: comments (before, between, inside an event), CRLF, UTF-8, duplicated finish_reason,
/// usage with cost/cost_details/is_byok, served model != requested.
pub fn text_stream() -> String {
    let mut s = String::from(COMMENT);
    s += &data(&chunk(
        json!({"role":"assistant","content":"h\u{e9}llo "}),
        Value::Null,
        json!({"provider":"Anthropic"}),
    ));
    s += COMMENT;
    // comment line *inside* an event, CRLF line endings
    let c2 = chunk(
        json!({"content":"w\u{f6}rld \u{1F600}"}),
        Value::Null,
        json!({}),
    );
    s += &format!("data: {c2}\r\n: keep-alive inside an event\r\n\r\n");
    s += COMMENT;
    s += &data(&chunk(json!({"content":""}), json!("stop"), json!({})));
    // final usage chunk repeats finish_reason in a content-free choice
    s += &data(&chunk(
        json!({"content":""}),
        json!("stop"),
        json!({"usage":{"prompt_tokens":5,"completion_tokens":4,"total_tokens":9,"cost":0.0001,"is_byok":true,
            "cost_details":{"upstream_inference_cost":0.00008,"upstream_inference_prompt_cost":0.00002,"upstream_inference_completions_cost":0.00006},
            "prompt_tokens_details":{"cached_tokens":1},"completion_tokens_details":{"reasoning_tokens":0}}}),
    ));
    s += "data: [DONE]\n\n";
    s
}

pub fn tool_fragment(index: usize, id: Option<&str>, name: Option<&str>, args: &str) -> String {
    let mut c = json!({"index": index, "function": {"arguments": args}});
    if let Some(id) = id {
        c["id"] = id.into();
        c["type"] = "function".into();
    }
    if let Some(n) = name {
        c["function"]["name"] = n.into();
    }
    data(&chunk(json!({"tool_calls":[c]}), Value::Null, json!({})))
}
/// Case 3: two parallel calls interleaved by index, id/name only on the first fragment, arguments
/// split mid-token and mid-UTF-8-character (the e-acute is whole in JSON, the split is between tokens).
pub fn tools_stream() -> String {
    let mut s = String::from(COMMENT);
    s += &tool_fragment(0, Some("call_a"), Some("add"), "");
    s += &tool_fragment(1, Some("call_b"), Some("echo"), "{\"te");
    s += &tool_fragment(0, None, None, "{\"a\": 1, ");
    s += &tool_fragment(0, None, None, "\"b\": 2}");
    s += &tool_fragment(1, None, None, "xt\": \"h\u{e9}\"}");
    s += &data(&chunk(
        json!({"content":""}),
        json!("tool_calls"),
        json!({"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}}),
    ));
    s += "data: [DONE]\n\n";
    s
}

/// Case 2: partial text, then the documented mid-stream error shape (top-level `error` plus a choice
/// with finish_reason "error"), then a chunk after the error.
pub fn mid_error_stream(trailing_chunk: bool) -> String {
    let mut s = data(&chunk(
        json!({"content":"partial "}),
        Value::Null,
        json!({}),
    ));
    s += COMMENT;
    s += &data(&chunk(
        json!({"content":""}),
        json!("error"),
        json!({"error":{"code":429,"message":"Rate limit exceeded","metadata":{"error_type":"rate_limit_exceeded","provider_code":"429"}}}),
    ));
    if trailing_chunk {
        s += &data(&chunk(
            json!({"content":"ignored after error"}),
            Value::Null,
            json!({}),
        ));
    }
    s
}
/// An error object alone in a frame (no chunk envelope).
pub fn bare_error_stream() -> String {
    let mut s = data(&chunk(
        json!({"content":"partial "}),
        Value::Null,
        json!({}),
    ));
    s += &data(
        &json!({"error":{"code":502,"message":"upstream down","metadata":{"error_type":"provider_overloaded"}}}),
    );
    s += &data(&chunk(
        json!({"content":"ignored after error"}),
        Value::Null,
        json!({}),
    ));
    s
}
