//! Runtime test: drive the model-call callback future on futures-executor (LocalPool, ThreadPool)
//! with no tokio runtime entered by the test, and again with a tokio runtime present.
//! Every scenario runs on its own thread with a watchdog, so a panic, an error and a hang are told apart.
mod fixtures;
mod support;
use fixtures::TEXT;
use futures_executor::{LocalPool, ThreadPool};
use futures_util::task::{LocalSpawnExt, SpawnExt};
use openrouter_genai::*;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use support::*;

#[derive(Debug)]
enum Outcome {
    Ok(String),
    #[allow(dead_code)]
    Err(String),
    Panic(String),
    Hang,
}
static LAST_PANIC: OnceLock<Mutex<String>> = OnceLock::new();
fn install_hook() {
    LAST_PANIC.get_or_init(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let msg = info.to_string().lines().last().unwrap_or("").to_string();
            *LAST_PANIC.get().unwrap().lock().unwrap() = msg;
            let _ = &prev; // keep the default hook quiet: the outcome is reported by the test
        }));
        Mutex::new(String::new())
    });
}
fn mk(m: &Mock, owned: bool) -> Arc<OpenRouterGenai> {
    let a = OpenRouterGenai::new(Config { base_url: m.base_url(), api_key: "k".into(), model: "openai/gpt-4o".into() });
    Arc::new(if owned { a.with_owned_runtime() } else { a })
}
fn req() -> ModelRequest {
    ModelRequest { messages: vec![Message::User("hi".into())], ..Default::default() }
}
fn outcome(r: Result<Result<ModelReply, String>, mpsc::RecvTimeoutError>) -> Outcome {
    match r {
        Ok(Ok(rep)) => Outcome::Ok(rep.text),
        Ok(Err(e)) => Outcome::Err(e),
        Err(mpsc::RecvTimeoutError::Disconnected) => Outcome::Panic(LAST_PANIC.get().unwrap().lock().unwrap().clone()),
        Err(mpsc::RecvTimeoutError::Timeout) => Outcome::Hang,
    }
}
const WAIT: Duration = Duration::from_secs(8);

/// With `handle`, the driving thread enters that tokio runtime's context first (the runtime itself
/// runs elsewhere; the future is still polled by futures-executor).
fn local_pool(owned: bool, handle: Option<tokio::runtime::Handle>) -> Outcome {
    install_hook();
    let m = Mock::start(Script::sse(TEXT, 40));
    let cb = model_callback(mk(&m, owned));
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _guard = handle.as_ref().map(|h| h.enter());
        let mut pool = LocalPool::new();
        let sp = pool.spawner();
        sp.spawn_local(async move {
            tx.send(cb(req()).await).ok();
        })
        .unwrap();
        pool.run();
    });
    outcome(rx.recv_timeout(WAIT))
}
fn thread_pool(owned: bool, handle: Option<tokio::runtime::Handle>) -> Outcome {
    install_hook();
    let m = Mock::start(Script::sse(TEXT, 40));
    let cb = model_callback(mk(&m, owned));
    let mut b = ThreadPool::builder();
    b.pool_size(2);
    if let Some(h) = handle {
        let h: &'static tokio::runtime::Handle = Box::leak(Box::new(h));
        // The guard is leaked on purpose: dropping it at thread exit, after the runtime is gone, aborts the process.
        b.after_start(move |_| std::mem::forget(h.enter()));
    }
    let pool = b.create().unwrap();
    let (tx, rx) = mpsc::channel();
    pool.spawn(async move {
        tx.send(cb(req()).await).ok();
    })
    .unwrap();
    outcome(rx.recv_timeout(WAIT))
}
fn report(name: &str, o: &Outcome) {
    println!("RUNTIME {name:<58} => {o:?}");
}

// ---- no tokio runtime entered ------------------------------------------------------------------
#[test]
fn a_local_pool_without_tokio() {
    let o = local_pool(false, None);
    report("LocalPool, no tokio", &o);
    assert!(matches!(o, Outcome::Panic(ref m) if m.contains("no reactor running")), "{o:?}");
}
#[test]
fn b_thread_pool_without_tokio() {
    let o = thread_pool(false, None);
    report("ThreadPool, no tokio", &o);
    assert!(matches!(o, Outcome::Panic(ref m) if m.contains("no reactor running")), "{o:?}");
}

// ---- with tokio ----------------------------------------------------------------------------------
#[tokio::test(flavor = "multi_thread")]
async fn c_awaited_inside_tokio() {
    let m = Mock::start(Script::sse(TEXT, 40));
    let cb = model_callback(mk(&m, false));
    let r = cb(req()).await;
    report("awaited on a tokio task", &Outcome::Ok(r.as_ref().unwrap().text.clone()));
    assert_eq!(r.unwrap().text, "h\u{e9}llo w\u{f6}rld \u{1F600}");
}
#[test]
fn d_local_pool_with_a_tokio_handle_entered() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let h = rt.handle().clone();
    let o = local_pool(false, Some(h));
    report("LocalPool, tokio Handle entered on the driving thread", &o);
    assert!(matches!(o, Outcome::Ok(ref t) if t == "h\u{e9}llo w\u{f6}rld \u{1F600}"), "{o:?}");
}
#[test]
fn e_thread_pool_with_a_tokio_handle_entered_on_each_worker() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let o = thread_pool(false, Some(rt.handle().clone()));
    report("ThreadPool, tokio Handle entered per worker (after_start)", &o);
    assert!(matches!(o, Outcome::Ok(_)), "{o:?}");
}

// ---- workaround: the adapter owns a runtime --------------------------------------------------------
#[test]
fn f_local_pool_with_adapter_owned_runtime() {
    let o = local_pool(true, None);
    report("LocalPool, no tokio, adapter-owned runtime", &o);
    assert!(matches!(o, Outcome::Ok(_)), "{o:?}");
}
#[test]
fn g_thread_pool_with_adapter_owned_runtime() {
    let o = thread_pool(true, None);
    report("ThreadPool, no tokio, adapter-owned runtime", &o);
    assert!(matches!(o, Outcome::Ok(_)), "{o:?}");
}
