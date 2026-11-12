//! Every case through every transport on every executor. Expected failures are asserted too, so the
//! README table cannot drift from the code: `cargo test -p conformance`.
use conformance::exec::{drive, Exec};
use conformance::{run_case, CASES, T};
use std::sync::Arc;
use std::time::Duration;

fn transport(name: &str) -> T {
    match name {
        "std" => Arc::new(transport_std::StdTransport),
        "ureq" => Arc::new(transport_ureq::UreqTransport::new()),
        "ureq-naive" => Arc::new(transport_ureq::UreqTransport::naive()),
        "reqwest" => Arc::new(transport_reqwest::ReqwestTransport::new()),
        "reqwest-owned-rt" => Arc::new(transport_reqwest::ReqwestTransport::with_owned_runtime()),
        _ => unreachable!(),
    }
}
fn results(tn: &str, ex: Exec) -> Vec<(&'static str, Result<String, String>)> {
    CASES.iter().map(|(id, _)| (*id, drive(ex, Duration::from_secs(8), run_case(id, transport(tn))).and_then(|r| r))).collect()
}
fn all_pass(tn: &str, ex: Exec) {
    for (id, r) in results(tn, ex) {
        assert!(r.is_ok(), "{tn} on {}: case {id}: {r:?}", ex.name());
    }
}
macro_rules! passes {
    ($($name:ident: $tn:expr, $ex:expr;)*) => { $(#[test] fn $name() { all_pass($tn, $ex) })* };
}
passes! {
    std_localpool: "std", Exec::LocalPool;
    std_threadpool: "std", Exec::ThreadPool;
    std_tokio: "std", Exec::Tokio;
    ureq_localpool: "ureq", Exec::LocalPool;
    ureq_threadpool: "ureq", Exec::ThreadPool;
    ureq_tokio: "ureq", Exec::Tokio;
    reqwest_tokio: "reqwest", Exec::Tokio;
    reqwest_owned_rt_localpool: "reqwest-owned-rt", Exec::LocalPool;
    reqwest_owned_rt_threadpool: "reqwest-owned-rt", Exec::ThreadPool;
    reqwest_owned_rt_tokio: "reqwest-owned-rt", Exec::Tokio;
}
#[test]
fn reqwest_without_tokio_panics_in_every_case() {
    for ex in [Exec::LocalPool, Exec::ThreadPool] {
        for (id, r) in results("reqwest", ex) {
            let e = r.expect_err(id);
            assert!(e.starts_with("panic:"), "{} case {id}: {e}", ex.name());
        }
    }
    assert!(conformance::exec::panic_log().iter().any(|m| m.contains("no reactor running")), "{:?}", conformance::exec::panic_log());
}
#[test]
fn naive_ureq_cannot_cancel_a_blocked_read() {
    let failed: Vec<_> = results("ureq-naive", Exec::LocalPool).into_iter().filter(|(_, r)| r.is_err()).map(|(id, _)| id).collect();
    assert_eq!(failed, ["2", "6"]);
}
