//! 64 concurrent sessions: a blocking transport costs one OS thread per in-flight call; reqwest on tokio costs none.
use conformance::exec::Exec;
use conformance::{threads, T};
use std::sync::{Arc, Mutex};

// The thread census is process-wide, so the two tests must not overlap.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

#[test]
fn std_transport_64_sessions_one_thread_each_on_a_single_threaded_executor() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let t: T = Arc::new(transport_std::StdTransport);
    let r = threads::run(t, Exec::LocalPool, 64);
    assert_eq!((r.ok, r.server_peak_active), (64, 64), "{r:?}");
    assert!(r.peak_worker_threads >= 60, "{r:?}");
}
#[test]
fn reqwest_64_sessions_no_worker_threads() {
    let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let t: T = Arc::new(transport_reqwest::ReqwestTransport::new());
    let r = threads::run(t, Exec::Tokio, 64);
    assert_eq!((r.ok, r.peak_worker_threads), (64, 0), "{r:?}");
}
