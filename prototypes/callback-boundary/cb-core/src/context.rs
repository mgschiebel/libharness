//! R2: a context object the host creates, registers names into, and the library holds
//! between calls (Lua-style). Per context, never global.
//!
//! Three ways a run can use it; the first is the only one that keeps "runs share nothing"
//! (ADR 0004):
//!   `snapshot_run`  copy the registrations at run start (one `Arc` clone); later `register`
//!                   calls affect only later runs.
//!   `live_run`      look the callbacks up in the context on every call. A `register` made
//!                   while a run is in flight changes what that run calls.
use crate::b1::*;
use crate::link::*;
use alloc::string::String;
use alloc::sync::Arc;
use call_core::*;
use serde_json::Value;
use spin::Mutex;

#[derive(Clone)]
pub struct Context(Arc<Mutex<Caps1>>);

impl Context {
    pub fn new(result_schema: Value, validate: impl Fn(&Value) -> Result<(), String> + Send + Sync + 'static) -> Self {
        Context(Arc::new(Mutex::new(Caps1::new(result_schema, validate))))
    }
    /// Registration goes through the same builder as R1; the context just holds the result.
    /// `replace`: a later registration under the same name wins (Lua-style `set`).
    pub fn update(&self, f: impl FnOnce(Caps1) -> Caps1) {
        let mut g = self.0.lock();
        let cur = g.clone();
        *g = f(cur);
    }
    pub fn replace_tool(&self, name: &str, f: ToolFn1) {
        let mut g = self.0.lock();
        let mut c = g.clone();
        let me = Arc::make_mut(&mut c.0);
        me.tools.retain(|(n, _)| n != name);
        me.tools.push((String::from(name), f));
        *g = c;
    }
    pub fn snapshot(&self) -> Caps1 {
        self.0.lock().clone()
    }
    pub fn snapshot_run(&self, token: CancelToken, sink: Sink, inbox: Inbox) -> (Run1, Spec) {
        let caps = self.snapshot();
        let spec = caps.spec().clone();
        (Run1::new(caps, token, sink, inbox), spec)
    }
    pub fn live_run(&self, token: CancelToken, sink: Sink, inbox: Inbox) -> (Run1, Spec) {
        let spec = self.snapshot().spec().clone(); // the model's tool list is fixed at run start...
        let me = self.clone();
        (Run1 { src: Arc::new(move || me.snapshot()), token, sink, inbox }, spec) // ...the callbacks are not
    }
}
