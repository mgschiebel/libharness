//! What the gate cannot see by itself: the gated crate starts no threads, uses no std, no
//! unsafe, and names no runtime. (`cargo check -p cb-core --target wasm32v1-none` is the gate
//! proper; it is run from the shell, see the README.)
#[test]
fn the_gated_core_has_no_std_no_threads_no_unsafe_no_runtime() {
    let dir = format!("{}/../cb-core/src", env!("CARGO_MANIFEST_DIR"));
    let banned = ["std::", "thread", "spawn", "tokio", "futures::", "unsafe ", "Instant", "SystemTime", "sleep"];
    let mut hits = vec![];
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let src = std::fs::read_to_string(&p).unwrap();
        for (i, l) in src.lines().enumerate() {
            let code = l.split("//").next().unwrap();
            if l.trim_start().starts_with("//") {
                continue;
            }
            for b in banned {
                if code.contains(b) && !code.contains("forbid(unsafe_code)") {
                    hits.push(format!("{}:{}: {}", p.file_name().unwrap().to_string_lossy(), i + 1, l.trim()));
                }
            }
        }
    }
    assert!(hits.is_empty(), "{hits:#?}");
    let lib = std::fs::read_to_string(format!("{}/../cb-core/src/lib.rs", env!("CARGO_MANIFEST_DIR"))).unwrap();
    assert!(lib.contains("#![no_std]") && lib.contains("#![forbid(unsafe_code)]"));
}

/// Dropping a Transport body must release the connection while everything else (the link,
/// the host's wire) is still alive: the close is the drop, not the end of the run.
#[test]
fn dropping_the_body_closes_the_socket_while_the_link_lives() {
    use cb_core::b1::*;
    use cb_core::b2::*;
    use cb_core::*;
    use cb_host::host::*;
    use cb_host::mock_net::*;
    use cb_host::rig::*;
    use cb_host::wire::*;
    use futures::executor::block_on;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    let (_s, model) = scripted(vec![]);
    let mut hc = basic_caps(model, Default::default());
    hc.transport = Some(Arc::new(|req, _| Box::pin(async move { connect(&req) })));
    for b2 in [false, true] {
        let srv = serve(word_chunks(60, 20));
        let tok = CancelToken::new();
        let obs = Obs::default();
        let link: Box<dyn Link> = if b2 {
            let wire = CountingWire { inner: HostWire::new(hc.clone(), obs.sink(), Inbox::default()), meter: Default::default() };
            Box::new(Link2 { wire: Arc::new(wire), token: tok.clone() })
        } else {
            Box::new(Run1::new(Rig::caps1(&hc), tok.clone(), obs.sink(), Inbox::default()))
        };
        // read one chunk to completion (no read in flight afterwards), then drop the body
        let t = block_on(async {
            let (head, mut body) = link.transport(HttpReq { url: srv.url.clone(), body: "{}".into() }).await.unwrap();
            assert_eq!(head.status, 200);
            assert!(body.next().await.is_some());
            let t = Instant::now();
            drop(body);
            t
        });
        std::thread::sleep(Duration::from_millis(100)); // the link (and the wire, and its body table) are still alive here
        let closed = srv.log.client_closed_at.lock().unwrap().expect("server saw the hang-up before the link was dropped");
        assert!(closed.saturating_duration_since(t) < Duration::from_millis(60));
        drop(link);
    }
}
