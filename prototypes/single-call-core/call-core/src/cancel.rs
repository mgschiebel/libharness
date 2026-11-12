//! Cancellation token: an atomic flag, a list of wakers, a list of synchronous hooks.
//! `core` + `alloc` + `spin` only; no timers, no channels, no executor. Same design as
//! session-task's token (reused, we wrote it); `std::sync::Mutex` became `spin::Mutex`
//! so it builds without std. Never held across an `.await`.
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll, Waker};
use spin::Mutex;

#[derive(Default)]
struct Inner {
    flag: AtomicBool,
    st: Mutex<St>,
}
#[derive(Default)]
struct St {
    wakers: Vec<Waker>,
    hooks: Vec<Box<dyn FnOnce() + Send>>,
}

/// Handed to every callback (model, tool, spawn). Cheap to clone.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<Inner>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.flag.load(Ordering::SeqCst)
    }
    /// Idempotent. Runs hooks synchronously on the calling thread, then wakes waiters.
    pub fn cancel(&self) {
        if self.0.flag.swap(true, Ordering::SeqCst) {
            return;
        }
        let (wakers, hooks) = {
            let mut st = self.0.st.lock();
            (core::mem::take(&mut st.wakers), core::mem::take(&mut st.hooks))
        };
        for h in hooks {
            h();
        }
        for w in wakers {
            w.wake();
        }
    }
    /// Register a synchronous hook (e.g. `child_handle.abort()`). Runs immediately if
    /// already cancelled. Hooks must not block.
    pub fn on_cancel(&self, f: impl FnOnce() + Send + 'static) {
        let mut st = self.0.st.lock();
        if self.is_cancelled() {
            drop(st);
            f();
        } else {
            st.hooks.push(Box::new(f));
        }
    }
    /// Future that completes when cancelled.
    pub fn cancelled(&self) -> Cancelled {
        Cancelled(self.clone())
    }
    /// A token that is cancelled when `self` is (but can also be cancelled alone).
    pub fn child(&self) -> CancelToken {
        let c = CancelToken::new();
        let c2 = c.clone();
        self.on_cancel(move || c2.cancel());
        c
    }
}

pub struct Cancelled(CancelToken);
impl Future for Cancelled {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0.is_cancelled() {
            return Poll::Ready(());
        }
        let mut st = (self.0).0.st.lock();
        if self.0.is_cancelled() {
            return Poll::Ready(());
        }
        if !st.wakers.iter().any(|w| w.will_wake(cx.waker())) {
            st.wakers.push(cx.waker().clone());
        }
        Poll::Pending
    }
}
