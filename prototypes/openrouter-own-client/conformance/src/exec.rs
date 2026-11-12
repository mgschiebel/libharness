//! Three ways to drive a future, with the failure mode of each recorded (ok / panic / hang).
use futures::executor::{LocalPool, ThreadPool};
use std::future::Future;
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::{Mutex, Once};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exec {
    LocalPool,
    ThreadPool,
    Tokio,
}
impl Exec {
    pub const ALL: [Exec; 3] = [Exec::LocalPool, Exec::ThreadPool, Exec::Tokio];
    pub fn name(self) -> &'static str {
        match self {
            Exec::LocalPool => "futures LocalPool",
            Exec::ThreadPool => "futures ThreadPool",
            Exec::Tokio => "tokio multi-thread",
        }
    }
}
static LAST_PANIC: Mutex<Option<String>> = Mutex::new(None);
static PANIC_LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Every panic message seen by the hook (tests run in parallel, so `drive`'s own copy can be overwritten).
pub fn panic_log() -> Vec<String> {
    PANIC_LOG.lock().unwrap().clone()
}
fn hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        std::panic::set_hook(Box::new(|i| {
            let m = i.to_string().replace('\n', " | ");
            PANIC_LOG.lock().unwrap().push(m.clone());
            *LAST_PANIC.lock().unwrap() = Some(m);
        }));
    });
}
/// Err("panic: ...") when the driving thread or task panics, Err("hang ...") past `limit`.
pub fn drive<R: Send + 'static>(exec: Exec, limit: Duration, fut: impl Future<Output = R> + Send + 'static) -> Result<R, String> {
    hook();
    *LAST_PANIC.lock().unwrap() = None;
    let (tx, rx) = channel();
    let mut keep = None; // the pool must outlive the task: dropping it stops the workers
    match exec {
        Exec::LocalPool => {
            std::thread::spawn(move || {
                let r = LocalPool::new().run_until(fut);
                let _ = tx.send(r);
            });
        }
        Exec::ThreadPool => {
            let pool = ThreadPool::builder().pool_size(2).create().unwrap();
            pool.spawn_ok(async move {
                let _ = tx.send(fut.await);
            });
            keep = Some(pool);
        }
        Exec::Tokio => {
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
                let _ = tx.send(rt.block_on(fut));
            });
        }
    }
    let out = rx.recv_timeout(limit);
    drop(keep);
    match out {
        Ok(r) => Ok(r),
        Err(RecvTimeoutError::Disconnected) => Err(format!("panic: {}", LAST_PANIC.lock().unwrap().clone().unwrap_or_default())),
        Err(RecvTimeoutError::Timeout) => Err(format!("hang (no result in {limit:?})")),
    }
}
