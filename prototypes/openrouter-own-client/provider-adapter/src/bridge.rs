//! Runtime-neutral bridge from a blocking worker thread to an async consumer. std only: a Mutex,
//! a Condvar and a stored `Waker`. This is the glue every blocking transport needs; it contains no
//! runtime type, so any executor can poll the futures it hands out.
//!
//! Cancel path: dropping the `Receiver` (which the `Body` owns) marks the channel cancelled, runs the
//! worker's registered cancel hook (e.g. `shutdown()` on the socket, which unblocks a read in
//! progress) and wakes a worker blocked on a full queue.
use crate::{Body, BoxFut};
use provider_codec::ResponseHead;
use std::collections::VecDeque;
use std::future::{poll_fn, Future};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Poll, Waker};

pub enum Item {
    Head(ResponseHead),
    Chunk(Vec<u8>),
    Fail(String),
}
type Hook = Box<dyn FnOnce() + Send>;
#[derive(Default)]
struct State {
    q: VecDeque<Item>,
    waker: Option<Waker>,
    rx_gone: bool,
    tx_gone: bool,
    on_cancel: Option<Hook>,
}
struct Shared {
    st: Mutex<State>,
    space: Condvar,
}
const CAP: usize = 8;

/// Worker side.
pub struct Sender(Arc<Shared>);
/// Async side. Dropping it cancels the worker.
pub struct Receiver(Arc<Shared>);

pub fn channel() -> (Sender, Receiver) {
    let sh = Arc::new(Shared { st: Mutex::new(State::default()), space: Condvar::new() });
    (Sender(sh.clone()), Receiver(sh))
}
impl Sender {
    /// Blocks while the queue is full (back-pressure). `false` means the consumer is gone: stop.
    pub fn send(&self, item: Item) -> bool {
        let mut st = self.0.st.lock().unwrap();
        while st.q.len() >= CAP && !st.rx_gone {
            st = self.0.space.wait(st).unwrap();
        }
        if st.rx_gone {
            return false;
        }
        st.q.push_back(item);
        let w = st.waker.take();
        drop(st);
        w.map(Waker::wake);
        true
    }
    /// Never blocks (ignores the capacity): for producers that live on an async runtime and must not park a thread.
    pub fn send_nowait(&self, item: Item) -> bool {
        let mut st = self.0.st.lock().unwrap();
        if st.rx_gone {
            return false;
        }
        st.q.push_back(item);
        let w = st.waker.take();
        drop(st);
        w.map(Waker::wake);
        true
    }
    /// Register what unblocks the worker if the consumer drops mid-read. Runs at once if already dropped.
    pub fn on_cancel(&self, f: impl FnOnce() + Send + 'static) {
        let mut st = self.0.st.lock().unwrap();
        if st.rx_gone {
            drop(st);
            f();
        } else {
            st.on_cancel = Some(Box::new(f));
        }
    }
}
impl Drop for Sender {
    fn drop(&mut self) {
        let mut st = self.0.st.lock().unwrap();
        st.tx_gone = true;
        let w = st.waker.take();
        drop(st);
        w.map(Waker::wake);
    }
}
impl Receiver {
    /// Same hook as `Sender::on_cancel`, for a producer that is a spawned task (hook = abort the task).
    pub fn on_cancel(&self, f: impl FnOnce() + Send + 'static) {
        let mut st = self.0.st.lock().unwrap();
        st.on_cancel = Some(Box::new(f));
    }
    /// `None` once the worker has finished and the queue is drained. The future owns an `Arc`, not a borrow.
    pub fn recv(&self) -> impl Future<Output = Option<Item>> + Send + 'static {
        let sh = self.0.clone();
        poll_fn(move |cx| {
            let mut st = sh.st.lock().unwrap();
            if let Some(i) = st.q.pop_front() {
                drop(st);
                sh.space.notify_all();
                return Poll::Ready(Some(i));
            }
            if st.tx_gone {
                return Poll::Ready(None);
            }
            st.waker = Some(cx.waker().clone());
            Poll::Pending
        })
    }
}
impl Drop for Receiver {
    fn drop(&mut self) {
        let mut st = self.0.st.lock().unwrap();
        st.rx_gone = true;
        let hook = st.on_cancel.take();
        drop(st);
        self.0.space.notify_all();
        hook.map(|f| f());
    }
}

struct BlockingBody(Receiver);
impl Body for BlockingBody {
    fn next_chunk(&mut self) -> BoxFut<'_, Option<Result<Vec<u8>, String>>> {
        let f = self.0.recv();
        Box::pin(async move {
            match f.await {
                Some(Item::Chunk(c)) => Some(Ok(c)),
                Some(Item::Fail(e)) => Some(Err(e)),
                Some(Item::Head(_)) => Some(Err("worker sent a second head".into())),
                None => None,
            }
        })
    }
}

/// Run a blocking HTTP exchange on its own named OS thread (spawned at first poll, so the future is
/// lazy like any other). The worker must send `Item::Head` first, then chunks; `Item::Fail` before
/// the head fails the call, after it fails the body. One thread per in-flight call.
pub fn spawn_call(name: &str, work: impl FnOnce(&Sender) + Send + 'static) -> BoxFut<'static, Result<(ResponseHead, Box<dyn Body>), String>> {
    let name = name.to_string();
    Box::pin(async move {
        let (tx, rx) = channel();
        std::thread::Builder::new().name(name).spawn(move || work(&tx)).map_err(|e| format!("spawn worker: {e}"))?;
        attach(rx).await
    })
}
/// Await the head from a producer (thread or task) and wrap the rest of the channel as a `Body`.
pub async fn attach(rx: Receiver) -> Result<(ResponseHead, Box<dyn Body>), String> {
    match rx.recv().await {
        Some(Item::Head(h)) => Ok((h, Box::new(BlockingBody(rx)) as Box<dyn Body>)),
        Some(Item::Fail(e)) => Err(e),
        Some(Item::Chunk(_)) => Err("producer sent a chunk before the head".into()),
        None => Err("producer ended without a response".into()),
    }
}

/// A timer with no runtime: one sleeping thread per call. A host-supplied `Sleep` for tests and
/// for hosts with no timer of their own.
pub fn thread_sleep(d: std::time::Duration) -> BoxFut<'static, ()> {
    let (tx, rx) = channel();
    let mut started = false;
    let mut tx = Some(tx);
    Box::pin(poll_fn(move |cx| {
        if !started {
            started = true;
            let tx = tx.take().unwrap();
            std::thread::Builder::new().name("lh-sleep".into()).spawn(move || {
                std::thread::sleep(d);
                drop(tx);
            }).expect("spawn sleeper");
        }
        let mut st = rx.0.st.lock().unwrap();
        if st.tx_gone {
            return Poll::Ready(());
        }
        st.waker = Some(cx.waker().clone());
        Poll::Pending
    }))
}
