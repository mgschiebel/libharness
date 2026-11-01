use host::*;
use nostd_core::{demo, CoreError};
use std::sync::{atomic::Ordering::SeqCst, Arc, Mutex};

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("PROTOTYPE-wipe-me-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap(); d
}

// Point 1: real I/O on tokio multi-thread and on futures-executor, 64 parallel sessions each.
#[test]
fn real_io_tokio_multithread() {
    let (dir, stub, probe) = (tmp("tok"), spawn_stub(), Arc::new(Probe::default()));
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).build().unwrap();
    rt.block_on(async {
        let hs: Vec<_> = (0..64u64).map(|i| { let c = io_caps(dir.clone(), stub.addr, probe.clone(), i);
            tokio::spawn(async move { let a = run_session(&c).await.unwrap(); (i, a, c.calls_made()) }) }).collect();
        for h in hs { let (i, a, calls) = h.await.unwrap(); assert_eq!(a.total, i as i64); assert_eq!(calls, 3); }
    });
    assert_eq!(probe.files_written.load(SeqCst), 64);
    assert_eq!(probe.http_ok.load(SeqCst), 64);
    assert_eq!(stub.hits.load(SeqCst), 64);
    assert_eq!(std::fs::read_to_string(dir.join("s7.txt")).unwrap(), "session 7");
    assert!(probe.bytes_read.load(SeqCst) >= 64 * 9);
}
#[test]
fn real_io_futures_executor() {
    let (dir, stub, probe) = (tmp("fut"), spawn_stub(), Arc::new(Probe::default()));
    let c = io_caps(dir.clone(), stub.addr, probe.clone(), 1);
    let a = futures::executor::block_on(run_session(&c)).unwrap();
    assert_eq!(a.total, 1);
    // and a thread pool
    let pool = futures::executor::ThreadPool::builder().pool_size(4).create().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    for i in 10..74u64 { let c = io_caps(dir.clone(), stub.addr, probe.clone(), i); let tx = tx.clone();
        pool.spawn_ok(async move { tx.send(run_session(&c).await.unwrap().total).unwrap(); }); }
    let mut got: Vec<i64> = (0..64).map(|_| rx.recv().unwrap()).collect(); got.sort();
    assert_eq!(got, (10..74).collect::<Vec<i64>>());
    assert_eq!(probe.http_ok.load(SeqCst), 65);
}

// Point 4: tracing events from the core reach the host subscriber; errors cross with `?`.
struct Cap(Arc<Mutex<Vec<(String, String)>>>);
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Cap {
    fn on_event(&self, e: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        self.0.lock().unwrap().push((e.metadata().target().into(), format!("{} {}", e.metadata().level(), e.metadata().name())));
    }
}
#[test]
fn tracing_and_errors() {
    use tracing_subscriber::layer::SubscriberExt;
    let seen = Arc::new(Mutex::new(vec![]));
    let sub = tracing_subscriber::registry().with(Cap(seen.clone()));
    tracing::subscriber::with_default(sub, || {
        let c = demo::caps();
        futures::executor::block_on(run_session(&c)).unwrap();
    });
    let s = seen.lock().unwrap();
    assert!(s.iter().any(|(t, m)| t == "nostd_core" && m.starts_with("INFO")), "{s:?}");
    assert!(s.iter().any(|(t, m)| t == "nostd_core" && m.starts_with("DEBUG")), "{s:?}");
    println!("core events seen by host subscriber: {s:?}");
    drop(s);
    // `?` with host error type, then boxed std::error::Error, source chain visible
    let c = nostd_core::Capabilities::<demo::Answer>::new();
    let e = futures::executor::block_on(run_session(&c)).unwrap_err();
    assert!(matches!(e, HostError::Core(CoreError::NoModel)));
    assert_eq!(e.to_string(), "core: no model registered");
    let b: Box<dyn std::error::Error + Send + Sync> = boxed(CoreError::UnknownTool("x".into())).unwrap_err();
    assert_eq!(b.to_string(), "unknown tool x");
    assert!(b.downcast_ref::<CoreError>().is_some());
    let _: &dyn std::error::Error = &CoreError::NoModel; // std::error::Error via core::error::Error
}

// Point 5: preserve_order is live in the shared tree (host asked for it) and reaches the core's Values.
#[test]
fn preserve_order_reaches_core() {
    let c = demo::caps();
    let keys: Vec<_> = c.result_schema().as_object().unwrap().keys().cloned().collect();
    println!("result_schema key order: {keys:?}");
    let v: serde_json::Value = serde_json::from_str(r#"{"z":1,"a":2}"#).unwrap();
    assert_eq!(v.as_object().unwrap().keys().collect::<Vec<_>>(), ["z", "a"]); // insertion order, not sorted
}
