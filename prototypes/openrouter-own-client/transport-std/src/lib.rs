//! Minimal HTTP/1.1 client on std sockets: the reference for "the host supplies the transport".
//! `http://` only (TLS, proxies, redirects, gzip, pooling and timeouts are all the host's problem).
//! Understands chunked, content-length and read-until-close bodies. One worker thread per call.
use provider_adapter::bridge::{spawn_call, Item, Sender};
use provider_adapter::provider_codec::{HttpRequest, ResponseHead};
use provider_adapter::{Body, BoxFut, Transport};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};

#[derive(Default, Clone)]
pub struct StdTransport;

impl Transport for StdTransport {
    fn send(&self, req: HttpRequest) -> BoxFut<'static, Result<(ResponseHead, Box<dyn Body>), String>> {
        spawn_call("lh-std-worker", move |tx| {
            if let Err(e) = run(&req, tx) {
                tx.send(Item::Fail(e));
            }
        })
    }
}

fn run(req: &HttpRequest, tx: &Sender) -> Result<(), String> {
    let rest = req.url.strip_prefix("http://").ok_or("transport-std speaks http:// only (TLS is the host's)")?;
    let (authority, path) = rest.split_once('/').map(|(a, p)| (a, format!("/{p}"))).unwrap_or((rest, "/".into()));
    let addr = if authority.contains(':') { authority.to_string() } else { format!("{authority}:80") };
    let mut s = TcpStream::connect(&addr).map_err(|e| format!("connect {addr}: {e}"))?;
    s.set_nodelay(true).ok();
    let killer = s.try_clone().map_err(|e| e.to_string())?;
    tx.on_cancel(move || {
        let _ = killer.shutdown(Shutdown::Both); // unblocks a read in progress, closes the connection
    });
    let mut head = format!("POST {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\nContent-Length: {}\r\n", req.body.len());
    for (k, v) in &req.headers {
        head += &format!("{k}: {v}\r\n");
    }
    s.write_all(head.as_bytes()).and_then(|_| s.write_all(b"\r\n")).and_then(|_| s.write_all(&req.body)).map_err(|e| format!("write: {e}"))?;
    let mut r = BufReader::new(s);
    let mut line = String::new();
    r.read_line(&mut line).map_err(|e| format!("read status: {e}"))?;
    let status: u16 = line.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| format!("bad status line {line:?}"))?;
    let mut headers = Vec::new();
    loop {
        line.clear();
        if r.read_line(&mut line).map_err(|e| format!("read header: {e}"))? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.trim_end().split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    let find = |n: &str| headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.to_ascii_lowercase());
    let chunked = find("transfer-encoding").map_or(false, |v| v.contains("chunked"));
    let len: Option<usize> = find("content-length").and_then(|v| v.parse().ok());
    if !tx.send(Item::Head(ResponseHead { status, headers })) {
        return Ok(());
    }
    let mut buf = [0u8; 8192];
    let mut pump = |r: &mut BufReader<TcpStream>, mut n: usize| -> Result<bool, String> {
        while n > 0 {
            let k = r.read(&mut buf[..n.min(8192)]).map_err(|e| format!("read body: {e}"))?;
            if k == 0 {
                return Err("connection closed inside a body".into());
            }
            n -= k;
            if !tx.send(Item::Chunk(buf[..k].to_vec())) {
                return Ok(false);
            }
        }
        Ok(true)
    };
    if chunked {
        loop {
            line.clear();
            if r.read_line(&mut line).map_err(|e| format!("read chunk size: {e}"))? == 0 {
                return Err("connection closed before the terminating chunk".into());
            }
            let size = usize::from_str_radix(line.trim().split(';').next().unwrap_or(""), 16).map_err(|_| format!("bad chunk size {line:?}"))?;
            if size == 0 {
                return Ok(()); // trailers ignored; Connection: close
            }
            if !pump(&mut r, size)? {
                return Ok(());
            }
            r.read_line(&mut line).ok(); // the CRLF after the chunk
        }
    } else if let Some(n) = len {
        pump(&mut r, n)?;
    } else {
        // until close
        loop {
            let k = r.read(&mut buf).map_err(|e| format!("read body: {e}"))?;
            if k == 0 || !tx.send(Item::Chunk(buf[..k].to_vec())) {
                return Ok(());
            }
        }
    }
    Ok(())
}
