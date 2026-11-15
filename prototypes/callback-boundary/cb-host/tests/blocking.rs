//! Blocking callbacks (axis K) and the host main thread. Siblings are two async tools that
//! each wait 50 ms on a host timer; the blocking tool sleeps 300 ms (standing in for a sync
//! file read or any blocking syscall) and is FIRST in the batch, the worst case for K1.
//! Metrics: sibling stall = last sibling's completion minus its nominal 50 ms; main-thread
//! block = longest gap in a 5 ms heartbeat on the host's main thread, minus the 5 ms.
use cb_core::b1::*;
use cb_core::*;
use cb_host::host::*;
use cb_host::rig::*;
use cb_host::scenarios::new_state;
use futures::executor::{block_on, LocalPool, ThreadPool};
use futures::task::LocalSpawnExt;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const BLOCK_MS: u64 = 300;
const SIB_MS: u64 = 50;
type Stamps = Arc<Mutex<Vec<Instant>>>;

fn ms(d: Duration) -> u128 {
    d.as_millis()
}
fn batch_model() -> HModel {
    let (_s, m) = scripted(vec![
        Turn::new(calls(vec![call("a", "slow", json!({})), call("b", "sib1", json!({})), call("c", "sib2", json!({}))])),
        Turn::new(answer(good())),
    ]);
    m
}
fn sib(stamps: &Stamps) -> impl Fn(ToolCtx1, Value) -> BoxFut<Result<Value, String>> + Send + Sync + 'static {
    let stamps = stamps.clone();
    move |_c, _v| {
        let stamps = stamps.clone();
        Box::pin(async move {
            delay(Duration::from_millis(SIB_MS)).await;
            stamps.lock().unwrap().push(Instant::now());
            Ok(json!("sib"))
        })
    }
}
fn blocking_body(_c: &ToolCtx1) -> Result<Value, String> {
    std::thread::sleep(Duration::from_millis(BLOCK_MS));
    Ok(json!("slow"))
}
fn caps1_base() -> Caps1 {
    let m = batch_model();
    Caps1::new(verdict_schema(), |v| serde_json::from_value::<Verdict>(v.clone()).map(|_| ()).map_err(|e| e.to_string()))
        .model(move |req, mc| m(req, Api::from_handles(mc.cancel, mc.sink)))
}
#[derive(Clone, Copy, Debug)]
enum K {
    /// K1: async-only registration; the blocking body is registered as an async tool anyway
    K1,
    /// K2: `tool_blocking` + a runner the host wrote (a plain `std::thread::spawn`, 3 lines)
    K2Spawn,
    /// K3: `tool_blocking` + the opt-in thread-runner crate (4 workers)
    K3Pool,
}
fn caps_k(k: K, stamps: &Stamps, body: fn(&ToolCtx1) -> Result<Value, String>) -> (Caps1, Option<thread_runner::ThreadRunner>) {
    let mut keep = None;
    let mut c = caps1_base();
    match k {
        K::K2Spawn => c = c.blocking_runner(Arc::new(|job| {
            std::thread::spawn(job);
        })),
        K::K3Pool => {
            let r = thread_runner::ThreadRunner::new(4);
            c = c.blocking_runner(r.runner());
            keep = Some(r);
        }
        K::K1 => {}
    }
    c = match k {
        K::K1 => c.tool("slow", "blocking body, registered async", json!({}), move |ctx, _| {
            let r = body(&ctx);
            Box::pin(async move { r })
        }),
        _ => c.tool_blocking("slow", "blocking", json!({}), move |ctx, _| body(&ctx)),
    }
    .unwrap();
    let c = c.tool("sib1", "", json!({}), sib(stamps)).unwrap().tool("sib2", "", json!({}), sib(stamps)).unwrap();
    (c, keep)
}
async fn run_caps(c: Caps1, token: CancelToken) -> (State, Result<Value, SessionError>) {
    let spec = c.spec().clone();
    drive(&Run1::new(c, token, Arc::new(|_| {}), Inbox::default()), &spec, new_state(1, "go")).await
}

/// Config M: the host drives the run on its main thread (one thread: a LocalPool) with a
/// heartbeat task beside it. Config P: the run is spawned on a pool; the main thread only waits.
fn on_main<F: std::future::Future<Output = ()>>(fut: F) -> u128 {
    let mut pool = LocalPool::new();
    let (done, gaps) = (Rc::new(Cell::new(false)), Rc::new(RefCell::new(vec![0u128])));
    let (d2, g2) = (done.clone(), gaps.clone());
    let mut last = Instant::now(); // the heartbeat "should have" ticked 5 ms after this instant
    pool.spawner()
        .spawn_local(async move {
            while !d2.get() {
                delay(Duration::from_millis(5)).await;
                g2.borrow_mut().push(ms(last.elapsed()));
                last = Instant::now();
            }
        })
        .unwrap();
    pool.run_until(async {
        fut.await;
        delay(Duration::from_millis(15)).await; // let the heartbeat task record a gap that ended after the run
        done.set(true)
    });
    let g = *gaps.borrow().iter().max().unwrap();
    g.saturating_sub(5)
}
fn on_pool(fut: impl std::future::Future<Output = ()> + Send + 'static) -> u128 {
    let pool = ThreadPool::builder().pool_size(2).create().unwrap();
    let (tx, rx) = mpsc::channel();
    pool.spawn_ok(async move {
        fut.await;
        let _ = tx.send(());
    });
    let (mut last, mut worst) = (Instant::now(), 0u128);
    while rx.recv_timeout(Duration::from_millis(5)).is_err() {
        worst = worst.max(ms(last.elapsed()));
        last = Instant::now();
    }
    worst.saturating_sub(5)
}
fn stall(stamps: &Stamps, t0: Instant) -> u128 {
    let g = stamps.lock().unwrap();
    assert_eq!(g.len(), 2, "both siblings finished");
    ms(g.iter().max().unwrap().duration_since(t0)).saturating_sub(SIB_MS as u128)
}
fn median(mut v: Vec<u128>) -> u128 {
    v.sort();
    v[v.len() / 2]
}

#[test]
fn k_axis_sibling_stall_and_main_thread_block() {
    println!("\n== K axis, B1 baseline, blocking tool {BLOCK_MS} ms first in a batch with two {SIB_MS} ms async siblings; median of 5 ==");
    for k in [K::K1, K::K2Spawn, K::K3Pool] {
        let (mut st_m, mut main_m, mut st_p, mut main_p, mut tot_m) = (vec![], vec![], vec![], vec![], vec![]);
        for _ in 0..5 {
            // config M
            let stamps: Stamps = Default::default();
            let (c, _keep) = caps_k(k, &stamps, blocking_body);
            let t0 = Instant::now();
            main_m.push(on_main(async {
                let (_, r) = run_caps(c, CancelToken::new()).await;
                assert!(r.is_ok());
            }));
            tot_m.push(ms(t0.elapsed()));
            st_m.push(stall(&stamps, t0));
            // config P
            let stamps: Stamps = Default::default();
            let (c, _keep) = caps_k(k, &stamps, blocking_body);
            let t0 = Instant::now();
            main_p.push(on_pool(async move {
                let (_, r) = run_caps(c, CancelToken::new()).await;
                assert!(r.is_ok());
            }));
            st_p.push(stall(&stamps, t0));
        }
        let (sm, mm, sp, mp, tm) = (median(st_m), median(main_m), median(st_p), median(main_p), median(tot_m));
        println!("  {k:?}: run on main thread (M): sibling stall {sm} ms, main-thread block {mm} ms, run {tm} ms | run on a pool (P): sibling stall {sp} ms, main-thread block {mp} ms");
        match k {
            K::K1 => assert!(sm >= 200 && mm >= 200 && sp >= 200, "K1 stalls the siblings and, on one thread, the host"),
            _ => assert!(sm <= 40 && mm <= 40 && sp <= 40, "K2/K3 keep siblings and the main thread free"),
        }
    }
}

#[test]
fn k_axis_across_boundaries() {
    println!("\n== blocking tool (as an async host callback that blocks) across boundaries, config M, median of 5 ==");
    for b in ALL {
        let (mut stl, mut mn) = (vec![], vec![]);
        for _ in 0..5 {
            let stamps: Stamps = Default::default();
            let s2 = stamps.clone();
            let mk = |stamps: Stamps| {
                move |_v: Value, _a: Api| -> BoxFut<Result<Value, String>> {
                    let stamps = stamps.clone();
                    Box::pin(async move {
                        delay(Duration::from_millis(SIB_MS)).await;
                        stamps.lock().unwrap().push(Instant::now());
                        Ok(json!("sib"))
                    })
                }
            };
            let hc = HostCaps::new(batch_model())
                .tool("slow", "blocks", |_v, _a| {
                    Box::pin(async {
                        std::thread::sleep(Duration::from_millis(BLOCK_MS));
                        Ok(json!("slow"))
                    })
                })
                .tool("sib1", "", mk(s2.clone()))
                .tool("sib2", "", mk(s2));
            let rig = Rig::new(b);
            let t0 = Instant::now();
            mn.push(on_main(async {
                let (_, r) = rig.run(&hc, new_state(1, "go"), RunIo::new(&Obs::default())).await;
                assert!(r.is_ok());
            }));
            stl.push(stall(&stamps, t0));
        }
        println!("  {b:?}: sibling stall {} ms, main-thread block {} ms", median(stl), median(mn));
    }
}

#[test]
fn k_registration_errors() {
    let r = caps1_base().tool_blocking("x", "", json!({}), |_c, _v| Ok(json!(1)));
    assert!(matches!(r, Err(RegisterError::NoBlockingRunner { ref tool }) if tool == "x"));
    // order matters: a runner registered after the blocking tool does not rescue it
    let r = caps1_base().tool_blocking("x", "", json!({}), |_c, _v| Ok(json!(1)));
    assert!(r.is_err());
    let ok = caps1_base().blocking_runner(Arc::new(|j| j())).tool_blocking("x", "", json!({}), |_c, _v| Ok(json!(1)));
    assert!(ok.is_ok());
    let dup = ok.unwrap().tool("x", "", json!({}), |_c, _v| Box::pin(async { Ok(json!(1)) }));
    assert!(matches!(dup, Err(RegisterError::Duplicate(_))));
}

/// What aborting does to a blocked tool. Abort is sent from another thread 50 ms in.
#[test]
fn k_axis_abort_releases_blocked_tool() {
    fn uncoop(_c: &ToolCtx1) -> Result<Value, String> {
        blocking_body(_c)
    }
    fn poll(c: &ToolCtx1) -> Result<Value, String> {
        for _ in 0..150 {
            if c.cancel.is_cancelled() {
                return Err("cancelled".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(json!("slow"))
    }
    fn hook(c: &ToolCtx1) -> Result<Value, String> {
        let (tx, rx) = mpsc::channel();
        c.cancel.on_cancel(move || {
            let _ = tx.send(());
        });
        match rx.recv_timeout(Duration::from_millis(BLOCK_MS)) {
            Ok(()) => Err("cancelled".into()),
            Err(_) => Ok(json!("slow")),
        }
    }
    println!("\n== abort 50 ms into a {BLOCK_MS} ms blocked tool: ms until the run ends / until the blocked thread is free (median of 5) ==");
    for k in [K::K1, K::K2Spawn, K::K3Pool] {
        for (name, body) in [("uncooperative", uncoop as fn(&ToolCtx1) -> Result<Value, String>), ("polls token", poll), ("token hook", hook)] {
            let (mut ends, mut frees) = (vec![], vec![]);
            for _ in 0..5 {
                static FREED: Mutex<Option<Instant>> = Mutex::new(None);
                // `body` is a plain fn pointer: wrap it so the release instant is recorded
                fn wrap(c: &ToolCtx1, f: fn(&ToolCtx1) -> Result<Value, String>) -> Result<Value, String> {
                    let r = f(c);
                    *FREED.lock().unwrap() = Some(Instant::now());
                    r
                }
                let stamps: Stamps = Default::default();
                let c = rebuild(k, body, &stamps, wrap);
                *FREED.lock().unwrap() = None;
                let tok = CancelToken::new();
                let t2 = tok.clone();
                let abort_at: Arc<Mutex<Option<Instant>>> = Default::default();
                let a2 = abort_at.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(50));
                    *a2.lock().unwrap() = Some(Instant::now());
                    t2.cancel();
                });
                let (tx, rx) = mpsc::channel();
                let pool = ThreadPool::builder().pool_size(2).create().unwrap();
                pool.spawn_ok(async move {
                    let (_, r) = run_caps(c, tok).await;
                    let _ = tx.send((Instant::now(), r));
                });
                let (ended, r) = rx.recv().unwrap();
                assert_eq!(r, Err(SessionError::Aborted));
                let t = abort_at.lock().unwrap().unwrap();
                std::thread::sleep(Duration::from_millis(BLOCK_MS + 50)); // let any uncooperative body finish
                let freed = FREED.lock().unwrap().expect("body returned");
                ends.push(ms(ended.duration_since(t)));
                frees.push(ms(freed.saturating_duration_since(t)));
            }
            println!("  {k:?} / {name:<13}: run ends {:>3} ms after abort, blocked thread free {:>3} ms after abort", median(ends), median(frees));
        }
    }
}
fn rebuild(k: K, body: fn(&ToolCtx1) -> Result<Value, String>, stamps: &Stamps, wrap: fn(&ToolCtx1, fn(&ToolCtx1) -> Result<Value, String>) -> Result<Value, String>) -> Caps1 {
    let mut c = caps1_base();
    if !matches!(k, K::K1) {
        c = c.blocking_runner(match k {
            K::K2Spawn => Arc::new(|job: Box<dyn FnOnce() + Send>| {
                std::thread::spawn(job);
            }) as Runner,
            _ => {
                let r = Box::leak(Box::new(thread_runner::ThreadRunner::new(4)));
                r.runner()
            }
        });
    }
    c = match k {
        K::K1 => c.tool("slow", "", json!({}), move |ctx, _| {
            let r = wrap(&ctx, body);
            Box::pin(async move { r })
        }),
        _ => c.tool_blocking("slow", "", json!({}), move |ctx, _| wrap(&ctx, body)),
    }
    .unwrap();
    c.tool("sib1", "", json!({}), sib(stamps)).unwrap().tool("sib2", "", json!({}), sib(stamps)).unwrap()
}

/// K3: the runner is bounded; capacity is the host's setting, and it is the one place a
/// blocking tool can still delay another blocking tool (never an async sibling).
#[test]
fn k3_pool_capacity() {
    println!("\n== K3 pool capacity: two 100 ms blocking tools and one 50 ms async sibling ==");
    for workers in [1usize, 2] {
        let stamps: Stamps = Default::default();
        let (_s, m) = scripted(vec![Turn::new(calls(vec![call("a", "b1", json!({})), call("b", "b2", json!({})), call("c", "sib1", json!({}))])), Turn::new(answer(good()))]);
        let r = thread_runner::ThreadRunner::new(workers);
        let c = Caps1::new(verdict_schema(), |v| serde_json::from_value::<Verdict>(v.clone()).map(|_| ()).map_err(|e| e.to_string()))
            .model(move |req, mc| m(req, Api::from_handles(mc.cancel, mc.sink)))
            .blocking_runner(r.runner());
        let slow100 = |_c: ToolCtx1, _v: Value| {
            std::thread::sleep(Duration::from_millis(100));
            Ok(json!(1))
        };
        let c = c.tool_blocking("b1", "", json!({}), slow100).unwrap().tool_blocking("b2", "", json!({}), slow100).unwrap().tool("sib1", "", json!({}), sib(&stamps)).unwrap();
        let t0 = Instant::now();
        let (_, res) = block_on(run_caps(c, CancelToken::new()));
        assert!(res.is_ok());
        let sib_ms = ms(stamps.lock().unwrap()[0].duration_since(t0));
        println!("  {workers} worker(s): batch finished in {} ms, async sibling finished at {} ms (nominal 50)", ms(t0.elapsed()), sib_ms);
        assert!(sib_ms < 90);
        if workers == 1 {
            assert!(ms(t0.elapsed()) >= 200)
        }
    }
}

/// K2 with a host that already has a blocking pool: tokio's `spawn_blocking` is the runner.
#[tokio::test(flavor = "current_thread")]
async fn k2_runner_is_tokio_spawn_blocking() {
    let stamps: Stamps = Default::default();
    let h = tokio::runtime::Handle::current();
    let c = caps1_base().blocking_runner(Arc::new(move |job| {
        h.spawn_blocking(job);
    }));
    let c = c.tool_blocking("slow", "", json!({}), |ctx, _| blocking_body(&ctx)).unwrap().tool("sib1", "", json!({}), sib(&stamps)).unwrap().tool("sib2", "", json!({}), sib(&stamps)).unwrap();
    let gaps = Arc::new(Mutex::new(vec![0u128]));
    let (g2, stop) = (gaps.clone(), Arc::new(std::sync::atomic::AtomicBool::new(false)));
    let s2 = stop.clone();
    let hb = tokio::spawn(async move {
        let mut last = Instant::now();
        while !s2.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
            g2.lock().unwrap().push(ms(last.elapsed()));
            last = Instant::now();
        }
    });
    let t0 = Instant::now();
    let (_, r) = run_caps(c, CancelToken::new()).await;
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    hb.await.unwrap();
    assert!(r.is_ok());
    let worst = gaps.lock().unwrap().iter().max().copied().unwrap().saturating_sub(5);
    println!("\n== K2, runner = tokio spawn_blocking, single-threaded tokio runtime ==\n  sibling stall {} ms, heartbeat block {} ms", stall(&stamps, t0), worst);
    assert!(worst < 60);
}

/// K3's sharp edge: abort ends the run at once, but an uncooperative blocked tool keeps its
/// worker. With a one-worker pool the next blocking tool waits for it.
#[test]
fn k3_an_aborted_uncooperative_tool_still_holds_its_worker() {
    let r = thread_runner::ThreadRunner::new(1);
    let mk = |secs: u64| {
        let (_s, m) = scripted(vec![Turn::new(calls(vec![call("a", "slow", json!({}))])), Turn::new(answer(good()))]);
        caps1_base().blocking_runner(r.runner()).model(move |req, mc| m(req, Api::from_handles(mc.cancel, mc.sink))).tool_blocking("slow", "", json!({}), move |_c, _v| {
            std::thread::sleep(Duration::from_millis(secs));
            Ok(json!(1))
        }).unwrap()
    };
    let tok = CancelToken::new();
    let t2 = tok.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        t2.cancel();
    });
    let t0 = Instant::now();
    let (_, a) = block_on(run_caps(mk(400), tok));
    assert_eq!(a, Err(SessionError::Aborted));
    let aborted_at = ms(t0.elapsed());
    let t1 = Instant::now();
    let (_, b) = block_on(run_caps(mk(10), CancelToken::new()));
    assert!(b.is_ok());
    let waited = ms(t1.elapsed());
    println!("\n== K3, 1 worker: run A aborted after {aborted_at} ms; run B's 10 ms blocking tool then took {waited} ms ==");
    assert!(waited >= 250);
}
