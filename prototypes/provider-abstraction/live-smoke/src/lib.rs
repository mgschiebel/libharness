//! PROTOTYPE (throwaway). HOST-SIDE half of the live smoke test: a blocking `ureq` client wrapped as
//! the adapter's `Transport`. This is exactly the part the libharness core must never contain. The API key
//! is only ever read from the environment by the tests and handed to the codec as a header value; it is
//! never printed or stored here.
use provider_adapter::provider_codec::{HttpRequest, ResponseHead};
use provider_adapter::{Body, BoxFut, Transport};
use std::io::Read;
use std::time::Duration;

pub struct UreqTransport {
    agent: ureq::Agent,
}
impl UreqTransport {
    pub fn new() -> Self {
        Self { agent: ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(15)).timeout_read(Duration::from_secs(120)).build() }
    }
}
impl Default for UreqTransport {
    fn default() -> Self {
        Self::new()
    }
}
struct ReaderBody(Option<Box<dyn Read + Send + Sync>>);
impl Body for ReaderBody {
    fn next_chunk(&mut self) -> BoxFut<Option<Result<Vec<u8>, String>>> {
        // Blocking read inside the future: fine for a smoke test driven by `block_on`; a real host
        // would use an async client. Dropping `ReaderBody` drops the reader = closes the connection.
        let mut buf = vec![0u8; 4096];
        let r = self.0.as_mut().map(|r| r.read(&mut buf));
        Box::pin(async move {
            match r {
                None | Some(Ok(0)) => None,
                Some(Ok(n)) => {
                    buf.truncate(n);
                    Some(Ok(buf))
                }
                Some(Err(e)) => Some(Err(e.to_string())),
            }
        })
    }
}
impl Transport for UreqTransport {
    fn send(&self, req: HttpRequest) -> BoxFut<Result<(ResponseHead, Box<dyn Body>), String>> {
        let agent = self.agent.clone();
        Box::pin(async move {
            let mut r = agent.post(&req.url);
            for (k, v) in &req.headers {
                r = r.set(k, v);
            }
            // ureq reports non-2xx as Err(Status); both arms carry a readable response.
            let resp = match r.send_bytes(&req.body) {
                Ok(resp) => resp,
                Err(ureq::Error::Status(_, resp)) => resp,
                Err(e) => return Err(format!("transport error: {e}")),
            };
            let headers = resp.headers_names().into_iter().filter_map(|n| resp.header(&n).map(|v| (n.clone(), v.to_string()))).collect();
            let head = ResponseHead { status: resp.status(), headers };
            let body: Box<dyn Body> = Box::new(ReaderBody(Some(resp.into_reader())));
            Ok((head, body))
        })
    }
}
