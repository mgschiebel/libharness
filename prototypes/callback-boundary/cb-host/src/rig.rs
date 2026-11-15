//! One entry point for the three boundaries: `Rig::run` and `Rig::exchange` take the host's
//! natural callbacks (`HostCaps`) and run the library over B1, B2 or B3. The code below the
//! `match` in each function is the glue the host (or the embedding) writes per boundary.
use crate::host::*;
use crate::wasm::*;
use crate::wire::*;
use cb_core::abi::*;
use cb_core::b1::*;
use cb_core::b2::*;
use cb_core::context::Context;
use cb_core::*;
use futures::channel::oneshot;
use futures::executor::ThreadPool;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boundary {
    B1,
    B2,
    B3Typed,
    B3Generic,
    /// B1 boundary, R2 registry: a `Context` the host registers into; the run snapshots it at start.
    B1CtxSnapshot,
    /// B1 boundary, R2 registry, naive: the run looks callbacks up in the context on every call.
    B1CtxLive,
}
pub const ALL: [Boundary; 4] = [Boundary::B1, Boundary::B2, Boundary::B3Typed, Boundary::B3Generic];
pub const R2: [Boundary; 2] = [Boundary::B1CtxSnapshot, Boundary::B1CtxLive];

#[derive(Clone)]
pub struct RunIo {
    pub token: CancelToken,
    pub inbox: Inbox,
    pub observer: Observer,
}
impl RunIo {
    pub fn new(obs: &Obs) -> RunIo {
        RunIo { token: CancelToken::new(), inbox: Inbox::default(), observer: obs.sink() }
    }
}
#[derive(Clone, Debug, Default)]
pub struct InstStats {
    pub instantiate_us: u128,
    pub memory_bytes: usize,
}

#[derive(Clone)]
pub struct Rig {
    pub b: Boundary,
    pub meter: Arc<Meter>,
    pub pool: ThreadPool,
    env: Option<Arc<WasmEnv>>,
    pub last_inst: Arc<Mutex<InstStats>>,
    /// Hand the host's own run token to callbacks (host-side only; nothing about it crosses).
    pub host_token: bool,
}
impl Rig {
    pub fn with_host_token(mut self) -> Rig {
        self.host_token = true;
        self
    }
    pub fn new(b: Boundary) -> Rig {
        let wasm = matches!(b, Boundary::B3Typed | Boundary::B3Generic);
        Rig {
            b,
            meter: Default::default(),
            pool: ThreadPool::builder().pool_size(4).create().unwrap(),
            env: wasm.then(WasmEnv::shared),
            last_inst: Default::default(),
            host_token: false,
        }
    }
    fn typed(&self) -> bool {
        self.b == Boundary::B3Typed
    }

    /// B1 glue: adapt the host's callbacks to registrations with handles.
    pub fn caps1(hc: &HostCaps) -> Caps1 {
        let m = hc.model.clone();
        let t = hc.transport.clone();
        let v = hc.validate.clone();
        let mut c = Caps1::new(hc.schema.clone(), move |x| v(x)).model(move |req, mc| m(req, Api::from_handles(mc.cancel, mc.sink)));
        if let Some(t) = t {
            c = c.transport(move |req, tok| t(req, Api::from_handles(tok, Arc::new(|_| {}))));
        }
        for (name, desc, f) in &hc.tools {
            let f = f.clone();
            c = c.tool(name, desc, json!({}), move |tc, args| f(args, Api::from_handles(tc.cancel, tc.sink))).unwrap();
        }
        c
    }
    /// R2 glue: create a context, register the host's callbacks into it, start a run from it.
    fn ctx_run(&self, hc: &HostCaps, io: RunIo) -> Run1 {
        let v = hc.validate.clone();
        let ctx = Context::new(hc.schema.clone(), move |x| v(x));
        ctx.update(|_| Self::caps1(hc));
        if self.b == Boundary::B1CtxLive { ctx.live_run(io.token, io.observer, io.inbox).0 } else { ctx.snapshot_run(io.token, io.observer, io.inbox).0 }
    }
    fn b2_link(&self, hc: &HostCaps, io: &RunIo) -> Link2 {
        let mut inner = HostWire::new(hc.clone(), io.observer.clone(), io.inbox.clone());
        inner.host_token = self.host_token.then(|| io.token.clone());
        let wire = CountingWire { inner, meter: self.meter.clone() };
        Link2 { wire: Arc::new(wire), token: io.token.clone() }
    }
    fn b3_state(&self, hc: &HostCaps, io: &RunIo) -> St {
        let mut w = HostWire::new(hc.clone(), io.observer.clone(), io.inbox.clone());
        w.host_token = self.host_token.then(|| io.token.clone());
        let wire = Arc::new(w);
        let (abort, notify) = (Arc::new(AtomicBool::new(false)), Arc::new(Notify::default()));
        let (a, n) = (abort.clone(), notify.clone());
        io.token.on_cancel(move || {
            a.store(true, SeqCst); // the only way a host abort reaches the guest: a flag it polls
            n.bump(); // plus a wake of its blocking `wait`
        });
        St::new(wire, self.pool.clone(), self.meter.clone(), abort, notify)
    }
    /// Run the guest on a thread of its own: it blocks that thread for the whole run.
    async fn guest(&self, st: St, job: Job, held: Option<Arc<Mutex<GuestInst>>>) -> GuestOut {
        let (env, typed, stats) = (self.env.clone().unwrap(), self.typed(), self.last_inst.clone());
        let (tx, rx) = oneshot::channel();
        std::thread::spawn(move || {
            let out = match held {
                None => {
                    let mut g = GuestInst::new(&env, st);
                    let out = g.run(typed, job);
                    *stats.lock().unwrap() = InstStats { instantiate_us: g.instantiate_time.as_micros(), memory_bytes: g.memory_bytes() };
                    out
                }
                Some(h) => {
                    let mut g = h.lock().unwrap(); // one run at a time per held instance
                    g.set_run(st);
                    g.run(typed, job)
                }
            };
            let _ = tx.send(out);
        });
        rx.await.unwrap()
    }

    pub async fn run(&self, hc: &HostCaps, state: State, io: RunIo) -> (State, Result<Value, SessionError>) {
        let spec = hc.spec();
        match self.b {
            Boundary::B1 => {
                let run = Run1::new(Self::caps1(hc), io.token, io.observer, io.inbox);
                drive(&run, &spec, state).await
            }
            Boundary::B2 => drive(&self.b2_link(hc, &io), &spec, state).await,
            Boundary::B1CtxSnapshot | Boundary::B1CtxLive => drive(&self.ctx_run(hc, io), &spec, state).await,
            _ => {
                let st = self.b3_state(hc, &io);
                match self.guest(st, Job::Loop { state, spec: Some(spec) }, None).await {
                    GuestOut::Loop { state, result } => (state, result),
                    _ => unreachable!(),
                }
            }
        }
    }
    pub async fn exchange(&self, hc: &HostCaps, req: HttpReq, io: RunIo) -> Option<Result<ModelReply, String>> {
        match self.b {
            Boundary::B1 => exchange(&Run1::new(Self::caps1(hc), io.token, io.observer, io.inbox), req).await,
            Boundary::B2 => exchange(&self.b2_link(hc, &io), req).await,
            Boundary::B1CtxSnapshot | Boundary::B1CtxLive => exchange(&self.ctx_run(hc, io), req).await,
            _ => {
                let st = self.b3_state(hc, &io);
                match self.guest(st, Job::Exchange { req }, None).await {
                    GuestOut::Exchange(r) => r,
                    _ => unreachable!(),
                }
            }
        }
    }

    /// R2 in Wasm: an instance the library holds between runs, registrations inside it.
    pub fn held(&self, hc: &HostCaps) -> Arc<Mutex<GuestInst>> {
        let io = RunIo { token: CancelToken::new(), inbox: Inbox::default(), observer: Arc::new(|_| {}) };
        let mut g = GuestInst::new(self.env.as_ref().unwrap(), self.b3_state(hc, &io));
        g.register(&hc.spec());
        Arc::new(Mutex::new(g))
    }
    pub async fn run_held(&self, held: &Arc<Mutex<GuestInst>>, hc: &HostCaps, state: State, io: RunIo) -> (State, Result<Value, SessionError>) {
        let st = self.b3_state(hc, &io);
        match self.guest(st, Job::Loop { state, spec: None }, Some(held.clone())).await {
            GuestOut::Loop { state, result } => (state, result),
            _ => unreachable!(),
        }
    }
}

/// Make a child's token follow the parent callback's. With a real token (B1) that is a sync
/// hook, instant; with plain callbacks (B2, B3) the host must poll `cancelled()` on a thread.
pub fn link_child(api: &Api) -> CancelToken {
    if let Some(t) = &api.token {
        return t.child();
    }
    let c = CancelToken::new();
    let (c2, api2) = (c.clone(), api.clone());
    std::thread::spawn(move || {
        while !c2.is_cancelled() {
            if api2.cancelled() {
                c2.cancel();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    });
    c
}
