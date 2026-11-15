//! Axis R: the capability set as a value the host passes per call (R1), or a context object
//! the library holds between calls (R2). Plus name-addressed calls in B3 (see boundary.rs and
//! measure.rs for the generic `call` import) and R2 inside a Wasm instance.
use cb_core::b1::*;
use cb_core::context::*;
use cb_core::*;
use cb_host::host::*;
use cb_host::rig::*;
use cb_host::scenarios::new_state;
use futures::channel::oneshot;
use futures::executor::block_on;
use futures::join;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn first_tool_result(st: &State, name: &str) -> String {
    st.messages.iter().find_map(|m| if let Message::ToolResult { name: n, content, .. } = m { (n == name).then(|| content.clone()) } else { None }).unwrap_or_default()
}
fn hc(model: HModel) -> HostCaps {
    basic_caps(model, Default::default())
}

#[test]
fn r1_a_value_the_library_does_not_keep() {
    let (_s, m) = scripted(vec![Turn::new(answer(good()))]);
    let caps = Rig::caps1(&hc(m));
    let t = Instant::now();
    let v: Vec<Caps1> = (0..100_000).map(|_| caps.clone()).collect();
    println!("\n== R1: 100000 clones of a 3-tool capability value took {} us (one Arc increment each) ==", t.elapsed().as_micros());
    drop(v);
    assert_eq!(caps.holders(), 1);
    let (run, spec) = {
        let run = Run1::new(caps.clone(), CancelToken::new(), Arc::new(|_| {}), Inbox::default());
        (run, caps.spec().clone())
    };
    assert_eq!(caps.holders(), 2, "the run holds exactly one clone while it exists");
    let (_, r) = block_on(drive(&run, &spec, new_state(1, "go")));
    assert!(r.is_ok());
    drop(run);
    assert_eq!(caps.holders(), 1, "after the run, the library holds nothing");
    // "changing" a value builds a new one; a run holding the old clone is unaffected
    let c2 = caps.clone().tool("extra", "", json!({}), |_c, _v| Box::pin(async { Ok(json!(1)) })).unwrap();
    assert_eq!((caps.spec().tools.len(), c2.spec().tools.len()), (3, 4));
}

/// R2: a context the library holds. The only form that keeps "runs share nothing" (ADR 0004)
/// snapshots at run start, and that is R1 with a mutable builder in front of it.
#[test]
fn r2_snapshot_isolates_and_live_lookup_leaks() {
    println!();
    for live in [false, true] {
        let (gate_tx, gate_rx) = oneshot::channel();
        let (started_tx, started_rx) = oneshot::channel();
        let (_s, m) = scripted(vec![
            Turn::new(calls(vec![call("c1", "add", json!({"a": 2, "b": 3})), call("c2", "extra", json!({}))])).held(gate_rx, started_tx),
            Turn::new(answer(good())),
        ]);
        let ctx = Context::new(verdict_schema(), |v| serde_json::from_value::<Verdict>(v.clone()).map(|_| ()).map_err(|e| e.to_string()));
        ctx.update(|_| Rig::caps1(&hc(m)));
        let (run, spec) = if live {
            ctx.live_run(CancelToken::new(), Arc::new(|_| {}), Inbox::default())
        } else {
            ctx.snapshot_run(CancelToken::new(), Arc::new(|_| {}), Inbox::default())
        };
        let ctx2 = ctx.clone();
        let drv = async move {
            started_rx.await.unwrap(); // run A is in flight, model call 1 not yet answered
            // the host re-registers "add" and adds "extra", through the context, while A runs
            ctx2.replace_tool("add", Arc::new(|_c, _v| Box::pin(async { Ok(json!(100)) })));
            ctx2.update(|c| c.tool("extra", "", json!({}), |_c, _v| Box::pin(async { Ok(json!("extra!")) })).unwrap());
            gate_tx.send(()).unwrap();
        };
        let ((st, r), ()) = block_on(async { join!(drive(&run, &spec, new_state(1, "go")), drv) });
        assert!(r.is_ok());
        let (add, extra) = (first_tool_result(&st, "add"), first_tool_result(&st, "extra"));
        println!("  R2 {}: in-flight run A saw add -> {add}, extra -> {extra:?}", if live { "live lookup  " } else { "snapshot     " });
        if live {
            assert_eq!(add, "100", "a registration made through the context changed a run already in flight");
            assert!(extra.contains("unknown"), "...yet a tool added mid-run is still invisible to the model (the spec was fixed at start)");
        } else {
            assert_eq!(add, "5");
        }
        // a run started afterwards sees the new registrations either way
        let (_s2, m2) = scripted(vec![Turn::new(calls(vec![call("c", "add", json!({"a": 1, "b": 1}))])), Turn::new(answer(good()))]);
        let m2c = m2.clone();
        ctx.update(|c| c.model(move |req, mc| m2c(req, Api::from_handles(mc.cancel, mc.sink))));
        let (run, spec) = ctx.snapshot_run(CancelToken::new(), Arc::new(|_| {}), Inbox::default());
        let (st, _) = block_on(drive(&run, &spec, new_state(2, "again")));
        assert_eq!(first_tool_result(&st, "add"), "100");
    }
}

#[test]
fn r2_contexts_are_independent_and_nothing_is_global() {
    let mk = |v: i64| {
        let (_s, m) = scripted(vec![Turn::new(calls(vec![call("c", "add", json!({}))])), Turn::new(answer(good()))]);
        let ctx = Context::new(verdict_schema(), |_| Ok(()));
        ctx.update(|_| Rig::caps1(&hc(m)));
        ctx.replace_tool("add", Arc::new(move |_c, _v| Box::pin(async move { Ok(json!(v)) })));
        ctx
    };
    let (a, b) = (mk(1), mk(2));
    let (ra, ia) = a.snapshot_run(CancelToken::new(), Arc::new(|_| {}), Inbox::default());
    let (rb, ib) = b.snapshot_run(CancelToken::new(), Arc::new(|_| {}), Inbox::default());
    let ((sa, _), (sb, _)) = block_on(async { join!(drive(&ra, &ia, new_state(1, "a")), drive(&rb, &ib, new_state(2, "b"))) });
    assert_eq!((first_tool_result(&sa, "add"), first_tool_result(&sb, "add")), ("1".to_string(), "2".to_string()));
    // the library source declares no global: no `static` item, no `thread_local`, no `lazy`
    for f in ["lib.rs", "link.rs", "b1.rs", "b2.rs", "context.rs", "abi.rs"] {
        let src = std::fs::read_to_string(format!("{}/../cb-core/src/{f}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        for l in src.lines().map(str::trim_start) {
            assert!(!(l.starts_with("static ") || l.starts_with("pub static ") || l.contains("thread_local") || l.contains("lazy_static")), "{f}: {l}");
        }
    }
}

/// R2 in Wasm: the instance holds the registrations, so it is the context. One instance runs
/// one thing at a time: a second run on the same context waits for the first.
#[test]
fn r2_in_wasm_one_instance_serialises_runs() {
    let rig = Rig::new(Boundary::B3Typed);
    let (a_gate, a_rx) = oneshot::channel();
    let (a_started, a_started_rx) = oneshot::channel();
    let (_sa, ma) = scripted(vec![Turn::new(answer(json!({"score": 1, "label": "A"}))).held(a_rx, a_started)]);
    let (_sb, mb) = scripted(vec![Turn::new(answer(json!({"score": 2, "label": "B"})))]);
    let (ha, hb) = (hc(ma), hc(mb));
    let held = rig.held(&ha); // registers the spec inside the instance once
    let t0 = Instant::now();
    let a = rig.run_held(&held, &ha, new_state(1, "a"), RunIo::new(&Obs::default()));
    let (r2, h2) = (rig.clone(), held.clone());
    let drv = async move {
        a_started_rx.await.unwrap();
        let t_b = Instant::now();
        let b = r2.run_held(&h2, &hb, new_state(2, "b"), RunIo::new(&Obs::default()));
        let release = async {
            futures::future::ready(()).await;
            std::thread::sleep(Duration::from_millis(150)); // B is queued behind A for at least this long
            a_gate.send(()).unwrap();
        };
        let (rb, ()) = join!(b, release);
        (t_b.elapsed().as_millis(), rb)
    };
    let ((_, ra), (b_waited_ms, (_, rb))) = block_on(async { join!(a, drv) });
    assert!(ra.is_ok() && rb.is_ok());
    println!("\n== R2 in Wasm: run B on the same held instance waited {b_waited_ms} ms for run A (total {} ms) ==", t0.elapsed().as_millis());
    assert!(b_waited_ms >= 150, "B could not start until A finished");
}
