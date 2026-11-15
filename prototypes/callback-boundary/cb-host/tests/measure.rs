//! Measurements for the README: crossings and bytes per run, Wasm instance costs, repeated
//! latencies. Run with `cargo test -p cb-host --test measure -- --nocapture --test-threads=1`.
use cb_host::host::*;
use cb_host::rig::*;
use cb_host::scenarios as sc;
use cb_host::wasm::WasmEnv;
use futures::executor::block_on;
use serde_json::json;

fn table(name: &str, rig: &Rig) {
    let (all, pay) = (rig.meter.total(), rig.meter.payload());
    println!("  {name:<28} total {:>4} crossings {:>6} B | payload {:>4} crossings {:>6} B", all.0, all.1, pay.0, pay.1);
    let mut rows: Vec<_> = rig.meter.snapshot().into_iter().collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.1 .0));
    println!("    by name: {}", rows.iter().map(|(k, v)| format!("{k}={}x/{}B", v.0, v.1)).collect::<Vec<_>>().join("  "));
}
fn bs() -> [Boundary; 3] {
    [Boundary::B2, Boundary::B3Typed, Boundary::B3Generic]
}

#[test]
fn crossings_and_bytes() {
    println!("\n== typed result + one tool (2 model calls, 1 tool, 1 validate) ==");
    for b in bs() {
        let rig = Rig::new(b);
        block_on(sc::typed_result(rig.clone()));
        table(&format!("{b:?}"), &rig);
    }
    println!("\n== model reply streamed, 6 deltas through the host sink ==");
    for b in bs() {
        let rig = Rig::new(b);
        block_on(sc::stream_deltas(rig.clone()));
        table(&format!("{b:?}"), &rig);
    }
    println!("\n== Transport call, 8-word streamed body (chunks of 64 bytes or less) ==");
    for b in bs() {
        let rig = Rig::new(b);
        block_on(sc::transport_stream(rig.clone()));
        table(&format!("{b:?}"), &rig);
    }
    println!("\n== Transport call cancelled after 3 deltas ==");
    for b in bs() {
        let rig = Rig::new(b);
        block_on(sc::transport_cancel(rig.clone()));
        table(&format!("{b:?}"), &rig);
    }
    println!("\n== five tool calls in one turn, tiny arguments ==");
    for b in bs() {
        let rig = Rig::new(b);
        let (_s, model) = scripted(vec![Turn::new(calls((0..5).map(|i| call(&format!("c{i}"), "add", json!({"a": i, "b": 1}))).collect())), Turn::new(answer(good()))]);
        let hc = basic_caps(model, Default::default());
        let (_, r) = block_on(rig.run(&hc, sc::new_state(1, "go"), RunIo::new(&Obs::default())));
        assert!(r.is_ok());
        table(&format!("{b:?}"), &rig);
    }
}

#[test]
fn wasm_instance_costs() {
    let env = WasmEnv::shared();
    println!("\n== B3 module: {} bytes, compiled once in {} ms ==", env.wasm_bytes, env.compile_time.as_millis());
    let (imp, exp) = env.surface();
    println!("  guest imports ({}): {}", imp.len(), imp.join(" "));
    println!("  guest exports ({}): {}", exp.len(), exp.join(" "));
    let rig = Rig::new(Boundary::B3Typed);
    for _ in 0..5 {
        block_on(sc::typed_result(rig.clone()));
        let s = rig.last_inst.lock().unwrap().clone();
        println!("  per-run instance: instantiate {} us, linear memory after the run {} KiB", s.instantiate_us, s.memory_bytes / 1024);
    }
}

fn spread(name: &str, mut v: Vec<u128>) {
    v.sort();
    println!("  {name:<34} min {:>4}  median {:>4}  max {:>4}  (ms, n={})", v[0], v[v.len() / 2], v[v.len() - 1], v.len());
}

#[test]
fn latencies_repeated() {
    println!("\n== latencies, 7 runs each (noisy: shared 2-core machine) ==");
    for b in ALL {
        let mut a = vec![];
        let mut hook = vec![];
        let mut ch = vec![];
        let mut tc = vec![];
        let mut fd = vec![];
        let mut rep = 0u32;
        let mut sch = vec![];
        let mut srep = 0u32;
        for _ in 0..7 {
            let (ms, h) = block_on(sc::abort_hang(Rig::new(b)));
            a.push(ms);
            hook.push(h as u128);
            let (c_ms, reported) = block_on(sc::cancel_reaches_child(Rig::new(b)));
            ch.push(c_ms);
            rep += reported as u32;
            let (s_ms, s_rep) = block_on(sc::cancel_reaches_spawned_child(Rig::new(b)));
            sch.push(s_ms);
            srep += s_rep as u32;
            tc.push(block_on(sc::transport_cancel(Rig::new(b))).0);
            fd.push(block_on(sc::stream_deltas(Rig::new(b))).0);
        }
        println!(" {b:?}: abort hook ran in {}/7; inline child reported Aborted to the parent's tool in {rep}/7, host-spawned child in {srep}/7", hook.iter().sum::<u128>());
        spread("abort -> hung tool released", a);
        spread("parent abort -> inline child freed", ch);
        spread("parent abort -> spawned child freed", sch);
        spread("cancel -> server saw hang-up", tc);
        spread("first delta latency (20 ms gap)", fd);
    }
}

/// With the host's own token handed to its callbacks (host side only), the polling thread and
/// the lost "Aborted" report go away for host-originated aborts.
#[test]
fn host_token_variant() {
    println!("\n== plain callbacks + the host's own token in `Api` (nothing crosses), 20 runs ==");
    for b in [Boundary::B2, Boundary::B3Typed] {
        let (mut inl, mut spw) = (0, 0);
        for _ in 0..20 {
            inl += block_on(sc::cancel_reaches_child(Rig::new(b).with_host_token())).1 as u32;
            spw += block_on(sc::cancel_reaches_spawned_child(Rig::new(b).with_host_token())).1 as u32;
        }
        println!("  {b:?}: inline child reported Aborted {inl}/20, host-spawned {spw}/20");
    }
}
