//! PROTOTYPE (throwaway, #27). Runs sessions on tokio multi-thread and futures-executor, with tracing.
use host::*;
use nostd_core::Capabilities;
use std::sync::{atomic::Ordering::SeqCst, Arc};
use std::time::Instant;

fn bench_tokio(shared: Arc<Capabilities<nostd_core::demo::Answer>>, sessions: usize, workers: usize) -> (f64, u64) {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(workers).build().unwrap();
    let t = Instant::now();
    rt.block_on(async {
        let hs: Vec<_> = (0..sessions).map(|_| { let c = shared.clone(); tokio::spawn(async move { run_session(&c).await.unwrap(); }) }).collect();
        for h in hs { h.await.unwrap(); }
    });
    (t.elapsed().as_secs_f64(), shared.calls_made())
}
fn bench_futures(shared: Arc<Capabilities<nostd_core::demo::Answer>>, sessions: usize, workers: usize) -> (f64, u64) {
    let pool = futures::executor::ThreadPool::builder().pool_size(workers).create().unwrap();
    let t = Instant::now();
    let (tx, rx) = std::sync::mpsc::channel();
    for _ in 0..sessions { let c = shared.clone(); let tx = tx.clone(); pool.spawn_ok(async move { run_session(&c).await.unwrap(); tx.send(()).unwrap(); }); }
    for _ in 0..sessions { rx.recv().unwrap(); }
    (t.elapsed().as_secs_f64(), shared.calls_made())
}
fn main() {
    let (sessions, k) = (64usize, 2000usize);
    let mut out = String::new();
    let mutex = if cfg!(feature = "std-mutex") { "std::sync::Mutex" } else { "spin::Mutex" };
    for workers in [1usize, 2, 8] {
        let s = Arc::new(cpu_caps(k));
        let (t, n) = bench_tokio(s, sessions, workers);
        out += &format!("{mutex} tokio     workers={workers} {sessions}x{k} calls={n} (expect {}) {:.3}s {:.0} calls/s\n", sessions * k, t, n as f64 / t);
        let s = Arc::new(cpu_caps(k));
        let (t, n) = bench_futures(s, sessions, workers);
        out += &format!("{mutex} futures   workers={workers} {sessions}x{k} calls={n} (expect {}) {:.3}s {:.0} calls/s\n", sessions * k, t, n as f64 / t);
    }
    print!("{out}");
    let _ = SeqCst;
}
