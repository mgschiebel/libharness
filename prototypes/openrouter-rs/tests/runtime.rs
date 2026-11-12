//! Runtime test: drive the callback future on futures-executor (LocalPool, ThreadPool) with no tokio
//! runtime entered, and again inside tokio. Each scenario runs on its own OS thread so a panic or a
//! hang is classified instead of taking the test process down.
mod common;
use common::*;
use futures_executor::{block_on, LocalPool, ThreadPool};
use futures_util::task::SpawnExt;
use openrouter_rs_adapter_proto::*;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug)]
enum Outcome {
    Ok(String),
    Err(String),
    Panic(String),
    Hang,
}
fn classify(f: impl FnOnce() -> Result<ModelReply, String> + Send + 'static) -> Outcome {
    static QUIET: std::sync::Once = std::sync::Once::new();
    QUIET.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if std::thread::current().name() != Some("classified") {
                default(info); // keep real failures loud; silence the panics we are classifying
            }
        }));
    });
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("classified".into())
        .spawn(move || {
            let o = match catch_unwind(AssertUnwindSafe(f)) {
                Ok(Ok(r)) => Outcome::Ok(r.text),
                Ok(Err(e)) => Outcome::Err(e),
                Err(p) => Outcome::Panic(
                    p.downcast_ref::<String>()
                        .cloned()
                        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_default(),
                ),
            };
            tx.send(o).ok();
        })
        .unwrap();
    rx.recv_timeout(Duration::from_secs(8))
        .unwrap_or(Outcome::Hang)
}
fn hi() -> ModelRequest {
    ModelRequest {
        messages: vec![Message::User("hi".into())],
        ..Default::default()
    }
}
fn mock() -> (Mock, Arc<OpenRouterAdapter>) {
    let m = Mock::start(vec![Reply::sse(text_stream(), 100_000)]);
    let a = Arc::new(OpenRouterAdapter::new(&m.base_url(), "k", REQUESTED).unwrap());
    (m, a)
}
const OK: &str = "h\u{e9}llo w\u{f6}rld \u{1F600}";

// ---- no tokio runtime entered ---------------------------------------------------------------
#[test]
fn a_construct_adapter_with_no_runtime() {
    let o = classify(|| {
        let (_m, _a) = mock();
        Ok(ModelReply {
            text: "built".into(),
            tool_calls: vec![],
            finish: FinishReason::Stop,
            usage: None,
            model: None,
        })
    });
    println!("A construct, no runtime -> {o:?}");
    assert!(matches!(o, Outcome::Ok(_)));
}
#[test]
fn b_local_pool_no_tokio() {
    let o = classify(|| {
        let (_m, a) = mock();
        let cb = model_callback(a);
        LocalPool::new().run_until(cb(hi()))
    });
    println!("B LocalPool, no tokio -> {o:?}");
    assert!(
        matches!(&o, Outcome::Panic(m) if m.contains("no reactor running")),
        "{o:?}"
    );
}
#[test]
fn c_thread_pool_no_tokio() {
    let o = classify(|| {
        let (_m, a) = mock();
        let cb = model_callback(a);
        let pool = ThreadPool::builder().pool_size(2).create().unwrap();
        let h = pool.spawn_with_handle(cb(hi())).unwrap();
        block_on(h)
    });
    println!("C ThreadPool, no tokio -> {o:?}");
    assert!(
        matches!(&o, Outcome::Panic(m) if m.contains("no reactor running")),
        "{o:?}"
    );
}
#[test]
fn d_block_on_no_tokio() {
    let o = classify(|| {
        let (_m, a) = mock();
        block_on(model_callback(a)(hi()))
    });
    println!("D futures block_on, no tokio -> {o:?}");
    assert!(
        matches!(&o, Outcome::Panic(m) if m.contains("no reactor running")),
        "{o:?}"
    );
}

// ---- inside tokio -----------------------------------------------------------------------------
#[test]
fn e_inside_tokio_multi_thread_and_current_thread() {
    for (name, rt) in [
        (
            "multi_thread",
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap(),
        ),
        (
            "current_thread",
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        ),
    ] {
        let o = classify(move || {
            let (_m, a) = mock();
            let cb = model_callback(a);
            rt.block_on(cb(hi()))
        });
        println!("E tokio {name} block_on -> {o:?}");
        assert!(matches!(&o, Outcome::Ok(t) if t == OK), "{o:?}");
    }
}
#[test]
fn f_foreign_executor_with_a_tokio_runtime_entered() {
    // A host that runs its session loop on futures-executor while a tokio runtime exists and is entered.
    for (name, rt) in [
        (
            "multi_thread",
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap(),
        ),
        (
            "current_thread",
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        ),
    ] {
        let o = classify(move || {
            let (_m, a) = mock();
            let cb = model_callback(a);
            let _guard = rt.enter();
            LocalPool::new().run_until(cb(hi()))
        });
        println!("F LocalPool + entered tokio {name} -> {o:?}");
        // multi_thread: its workers drive the connection task, so it works. current_thread: nobody
        // drives the runtime (the host's executor does not), so the connection task never runs.
        match name {
            "multi_thread" => assert!(matches!(&o, Outcome::Ok(t) if t == OK), "{o:?}"),
            _ => assert!(matches!(o, Outcome::Hang), "{o:?}"),
        }
    }
}

#[test]
fn c2_thread_pool_spawn_ok_panic_costs_a_worker() {
    // What the host sees with plain `spawn_ok` (no handle to carry the panic back): the single worker dies.
    let o = classify(|| {
        let (_m, a) = mock();
        let cb = model_callback(a);
        let pool = ThreadPool::builder().pool_size(1).create().unwrap();
        pool.spawn_ok(async move {
            let _ = cb(hi()).await;
        });
        std::thread::sleep(Duration::from_millis(500));
        let (tx, rx) = mpsc::channel();
        pool.spawn_ok(async move {
            tx.send(()).ok();
        });
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(()) => Ok(ModelReply {
                text: "pool survived".into(),
                tool_calls: vec![],
                finish: FinishReason::Stop,
                usage: None,
                model: None,
            }),
            Err(_) => {
                Err("pool of 1 never ran the next task: its only worker died with the panic".into())
            }
        }
    });
    println!("C2 ThreadPool spawn_ok, no tokio -> {o:?}");
    assert!(
        matches!(&o, Outcome::Err(m) if m.contains("worker died")),
        "{o:?}"
    );
}

// ---- workaround: the adapter owns a tokio runtime (feature `private-runtime`) --------------------
#[cfg(feature = "private-runtime")]
mod private_runtime {
    use super::*;
    use openrouter_rs_adapter_proto::private_runtime::model_callback_private_runtime;

    fn rt() -> Arc<tokio::runtime::Runtime> {
        Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .unwrap(),
        )
    }
    #[test]
    fn g_private_runtime_polled_by_futures_executors_with_no_tokio_entered() {
        let o = classify(|| {
            let (_m, a) = mock();
            let cb = model_callback_private_runtime(a, rt());
            LocalPool::new().run_until(cb(hi()))
        });
        println!("G private runtime, LocalPool -> {o:?}");
        assert!(matches!(&o, Outcome::Ok(t) if t == OK), "{o:?}");
        let o = classify(|| {
            let (_m, a) = mock();
            let cb = model_callback_private_runtime(a, rt());
            let pool = ThreadPool::builder().pool_size(2).create().unwrap();
            block_on(pool.spawn_with_handle(cb(hi())).unwrap())
        });
        println!("G private runtime, ThreadPool -> {o:?}");
        assert!(matches!(&o, Outcome::Ok(t) if t == OK), "{o:?}");
        let o = classify(|| {
            let (_m, a) = mock();
            block_on(model_callback_private_runtime(a, rt())(hi()))
        });
        println!("G private runtime, block_on -> {o:?}");
        assert!(matches!(&o, Outcome::Ok(t) if t == OK), "{o:?}");
    }
    #[test]
    fn h_private_runtime_drop_aborts_the_task_and_closes_the_socket() {
        let m = Mock::start(vec![Reply::sse(
            text_stream()
                .split("data: [DONE]")
                .next()
                .unwrap()
                .to_string(),
            100_000,
        )
        .hold()]);
        let a = Arc::new(OpenRouterAdapter::new(&m.base_url(), "k", REQUESTED).unwrap());
        let r = rt();
        let cb = model_callback_private_runtime(a, r.clone());
        let fut = cb(hi()); // never polled: the task was spawned eagerly
        assert!(
            m.wait_accepted(Duration::from_secs(5)),
            "request went out although the future was never polled"
        );
        assert_eq!(m.peer_closed.load(std::sync::atomic::Ordering::SeqCst), 0);
        drop(fut);
        assert!(
            m.wait_closed(Duration::from_secs(5)),
            "dropping the callback future aborted the task and closed the socket"
        );
    }
}
