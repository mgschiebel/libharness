//! Prints the results table: every case x every executor x every transport.
use conformance::exec::{drive, Exec};
use conformance::{run_case, threads, CASES, T};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
fn main() {
    let only: Vec<String> = std::env::args().skip(1).collect();
    let want = |n: &str| only.is_empty() || only.iter().any(|o| o == n);
    let mut notes = Vec::new();
    for tn in ["std", "ureq", "ureq-naive", "reqwest", "reqwest-owned-rt"] {
        if !want(tn) {
            continue;
        }
        for ex in Exec::ALL {
            let mut row = Vec::new();
            for (id, _) in CASES {
                let t0 = Instant::now();
                let r = drive(ex, Duration::from_secs(8), run_case(id, transport(tn))).and_then(|r| r);
                let ms = t0.elapsed().as_millis();
                row.push(format!("{id}:{}", if r.is_ok() { "pass" } else { "FAIL" }));
                notes.push(format!("{tn} | {} | {id} | {} | {ms} ms", ex.name(), match r { Ok(n) => n, Err(e) => format!("FAIL: {e}") }));
            }
            println!("{tn:8} {:20} {}", ex.name(), row.join(" "));
        }
    }
    println!("\n--- details ---");
    for n in notes {
        println!("{n}");
    }
    if want("threads") {
        println!("\n--- 64 sessions ---");
        for tn in ["std", "ureq", "ureq-naive", "reqwest", "reqwest-owned-rt"] {
            for ex in Exec::ALL {
                if let Ok(f) = std::env::var("TN") {
                    if f != tn {
                        continue;
                    }
                }
                if let Ok(f) = std::env::var("EX") {
                    if !ex.name().contains(&f) {
                        continue;
                    }
                }
                if tn == "reqwest" && ex != Exec::Tokio {
                    continue; // panics without a tokio runtime (see the table above)
                }
                let n: usize = std::env::var("N").ok().and_then(|v| v.parse().ok()).unwrap_or(64);
                let r = threads::run(transport(tn), ex, n);
                println!("{tn:8} {:20} {r:?}", ex.name());
            }
        }
    }
}
