//! B3: the wasmtime embedding. The guest (`cb-guest`) is the library's Loop and Exchange; the
//! imports below are the host's side of every callback. One instance per run (or one
//! long-lived instance per context for R2). Everything here is host code and is counted as glue.
use crate::wire::*;
use cb_core::abi::*;
use cb_core::b2::Wire;
use cb_core::*;
use futures::executor::ThreadPool;
use futures::future::{abortable, AbortHandle};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
use wasmtime::*;

#[derive(Default)]
pub struct Notify {
    gen: Mutex<u64>,
    cv: Condvar,
}
impl Notify {
    pub fn bump(&self) {
        *self.gen.lock().unwrap() += 1;
        self.cv.notify_all();
    }
}
struct Op {
    slot: Arc<Mutex<Option<Vec<u8>>>>,
    handle: AbortHandle,
    flag: Arc<AtomicBool>,
}
/// Per-run host state, stored in the wasmtime `Store`.
pub struct St {
    pub wire: Arc<HostWire>,
    pub pool: ThreadPool,
    pub meter: Arc<Meter>,
    pub abort: Arc<AtomicBool>,
    pub notify: Arc<Notify>,
    ops: HashMap<i32, Op>,
    next: i32,
    seen: u64,
}
impl St {
    pub fn new(wire: Arc<HostWire>, pool: ThreadPool, meter: Arc<Meter>, abort: Arc<AtomicBool>, notify: Arc<Notify>) -> Self {
        St { wire, pool, meter, abort, notify, ops: HashMap::new(), next: 1, seen: 0 }
    }
}

pub struct WasmEnv {
    engine: Engine,
    module: Module,
    linker: Linker<St>,
    pub compile_time: Duration,
    pub wasm_bytes: usize,
}
fn guest_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/wasm32v1-none/release/cb_guest.wasm")
}
impl WasmEnv {
    pub fn shared() -> Arc<WasmEnv> {
        static ENV: OnceLock<Arc<WasmEnv>> = OnceLock::new();
        ENV.get_or_init(|| {
            if !guest_path().exists() {
                let ok = std::process::Command::new("cargo")
                    .args(["build", "-p", "cb-guest", "--target", "wasm32v1-none", "--release"])
                    .current_dir(env!("CARGO_MANIFEST_DIR"))
                    .status()
                    .unwrap()
                    .success();
                assert!(ok, "could not build the guest");
            }
            Arc::new(WasmEnv::load(&std::fs::read(guest_path()).unwrap()))
        })
        .clone()
    }
    pub fn load(bytes: &[u8]) -> WasmEnv {
        let t = Instant::now();
        let engine = Engine::default();
        let module = Module::new(&engine, bytes).expect("compile guest");
        let compile_time = t.elapsed();
        let mut linker: Linker<St> = Linker::new(&engine);
        link_imports(&mut linker);
        WasmEnv { engine, module, linker, compile_time, wasm_bytes: bytes.len() }
    }
}

fn mem(c: &mut Caller<'_, St>) -> Memory {
    c.get_export("memory").and_then(|e| e.into_memory()).expect("guest memory")
}
fn read(c: &mut Caller<'_, St>, ptr: i32, len: i32) -> Vec<u8> {
    let m = mem(c);
    m.data(&c)[ptr as usize..(ptr + len) as usize].to_vec()
}
fn write(c: &mut Caller<'_, St>, ptr: i32, b: &[u8]) {
    let m = mem(c);
    m.write(c, ptr as usize, b).unwrap();
}
fn enc_res(r: Result<Vec<u8>, String>) -> Vec<u8> {
    match r {
        Ok(b) => [&[0u8][..], &b].concat(),
        Err(e) => [&[1u8][..], e.as_bytes()].concat(),
    }
}
/// Start a host callback as a task on the host's pool; return its op id at once.
fn start(c: &mut Caller<'_, St>, op: &str, args: Vec<u8>, name: &'static str, extra: usize) -> i32 {
    let st = c.data_mut();
    st.meter.cross(name, args.len() + extra);
    let id = st.next;
    st.next += 1;
    let (slot, flag) = (Arc::new(Mutex::new(None)), Arc::new(AtomicBool::new(false)));
    let (abort, f2, obs) = (st.abort.clone(), flag.clone(), st.wire.observer.clone());
    let plain = cb_core::b2::Plain {
        cancelled: Arc::new(move || f2.load(SeqCst) || abort.load(SeqCst)),
        emit: Arc::new(move |b| {
            if let Ok(e) = serde_json::from_slice::<Ev>(b) {
                obs(e)
            }
        }),
    };
    let (fut, handle) = abortable(st.wire.call(op, args, plain));
    let (s2, n2) = (slot.clone(), st.notify.clone());
    st.pool.spawn_ok(async move {
        if let Ok(r) = fut.await {
            *s2.lock().unwrap() = Some(enc_res(r));
            n2.bump();
        }
    });
    st.ops.insert(id, Op { slot, handle, flag });
    id
}

fn link_imports(l: &mut Linker<St>) {
    let m = "h";
    l.func_wrap(m, "call_start", |mut c: Caller<'_, St>, np: i32, nl: i32, ap: i32, al: i32| -> i32 {
        let name = String::from_utf8(read(&mut c, np, nl)).unwrap();
        let args = read(&mut c, ap, al);
        start(&mut c, &name, args, "call_start", name.len())
    })
    .unwrap();
    for (imp, op) in [("model_start", "model"), ("tool_start", "tool"), ("transport_start", "transport"), ("body_next_start", "body_next")] {
        l.func_wrap(m, imp, move |mut c: Caller<'_, St>, p: i32, n: i32| -> i32 {
            let args = read(&mut c, p, n);
            start(&mut c, op, args, imp_name(imp), 0)
        })
        .unwrap();
    }
    l.func_wrap(m, "op_poll", |c: Caller<'_, St>, op: i32| -> i32 {
        c.data().meter.cross("op_poll", 0);
        match c.data().ops.get(&op).and_then(|o| o.slot.lock().unwrap().as_ref().map(|b| b.len())) {
            Some(n) => n as i32,
            None => -1,
        }
    })
    .unwrap();
    l.func_wrap(m, "op_take", |mut c: Caller<'_, St>, op: i32, out: i32, cap: i32| -> i32 {
        let o = c.data_mut().ops.remove(&op).expect("op");
        let b = o.slot.lock().unwrap().take().expect("result");
        assert!(b.len() <= cap as usize);
        c.data().meter.cross("op_take", b.len());
        write(&mut c, out, &b);
        b.len() as i32
    })
    .unwrap();
    l.func_wrap(m, "op_drop", |mut c: Caller<'_, St>, op: i32| {
        c.data().meter.cross("op_drop", 0);
        if let Some(o) = c.data_mut().ops.remove(&op) {
            o.flag.store(true, SeqCst);
            o.handle.abort();
        }
    })
    .unwrap();
    l.func_wrap(m, "wait", |mut c: Caller<'_, St>| {
        c.data().meter.cross("wait", 0);
        let (n, seen) = (c.data().notify.clone(), c.data().seen);
        let mut g = n.gen.lock().unwrap();
        if *g == seen {
            g = n.cv.wait_timeout(g, Duration::from_millis(5)).unwrap().0; // 5 ms: also the poll interval for plain-callback cancel
        }
        let now = *g;
        drop(g);
        c.data_mut().seen = now;
    })
    .unwrap();
    l.func_wrap(m, "cancelled", |c: Caller<'_, St>| -> i32 {
        c.data().meter.cross("cancelled", 0);
        c.data().abort.load(SeqCst) as i32
    })
    .unwrap();
    l.func_wrap(m, "emit", |mut c: Caller<'_, St>, p: i32, n: i32| {
        let b = read(&mut c, p, n);
        c.data().meter.cross("emit", b.len());
        c.data().wire.emit(&b);
    })
    .unwrap();
    l.func_wrap(m, "validate", |mut c: Caller<'_, St>, p: i32, n: i32, err: i32, cap: i32| -> i32 {
        let v = read(&mut c, p, n);
        let r = c.data().wire.validate(&v);
        let msg = r.err().unwrap_or_default();
        let b = &msg.as_bytes()[..msg.len().min(cap as usize)];
        c.data().meter.cross("validate", v.len() + b.len());
        write(&mut c, err, b);
        b.len() as i32
    })
    .unwrap();
    l.func_wrap(m, "inbox", |mut c: Caller<'_, St>, out: i32, cap: i32| -> i32 {
        let b = c.data().wire.inbox();
        assert!(b.len() <= cap as usize, "inbox batch larger than the guest's buffer");
        c.data().meter.cross("inbox", b.len());
        write(&mut c, out, &b);
        b.len() as i32
    })
    .unwrap();
    l.func_wrap(m, "body_close", |c: Caller<'_, St>, h: i64| {
        c.data().meter.cross("body_close", 8);
        c.data().wire.close("body", h as u64);
    })
    .unwrap();
}
fn imp_name(s: &str) -> &'static str {
    match s {
        "model_start" => "model_start",
        "tool_start" => "tool_start",
        "transport_start" => "transport_start",
        _ => "body_next_start",
    }
}

impl WasmEnv {
    /// (imports, exports) of the guest module, as "module.name".
    pub fn surface(&self) -> (Vec<String>, Vec<String>) {
        (
            self.module.imports().map(|i| format!("{}.{}", i.module(), i.name())).collect(),
            self.module.exports().map(|e| e.name().to_string()).collect(),
        )
    }
}

pub struct GuestInst {
    store: Store<St>,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    run: TypedFunc<(i32, i32), i32>,
    out_ptr: TypedFunc<(), i32>,
    register: TypedFunc<(i32, i32), ()>,
    pub instantiate_time: Duration,
}
impl GuestInst {
    pub fn new(env: &WasmEnv, st: St) -> GuestInst {
        let t = Instant::now();
        let mut store = Store::new(&env.engine, st);
        let inst = env.linker.instantiate(&mut store, &env.module).expect("instantiate");
        let g = GuestInst {
            memory: inst.get_memory(&mut store, "memory").unwrap(),
            alloc: inst.get_typed_func(&mut store, "guest_alloc").unwrap(),
            run: inst.get_typed_func(&mut store, "guest_run").unwrap(),
            out_ptr: inst.get_typed_func(&mut store, "guest_out_ptr").unwrap(),
            register: inst.get_typed_func(&mut store, "guest_register").unwrap(),
            instantiate_time: t.elapsed(),
            store,
        };
        g
    }
    /// A held instance serves one run at a time; the next run replaces the per-run state.
    pub fn set_run(&mut self, st: St) {
        *self.store.data_mut() = st;
    }
    pub fn memory_bytes(&self) -> usize {
        self.memory.data_size(&self.store)
    }
    fn put(&mut self, b: &[u8]) -> i32 {
        let p = self.alloc.call(&mut self.store, b.len() as i32).unwrap();
        self.memory.write(&mut self.store, p as usize, b).unwrap();
        p
    }
    pub fn register(&mut self, spec: &Spec) {
        let b = serde_json::to_vec(spec).unwrap();
        self.store.data().meter.cross("export:register", b.len());
        let p = self.put(&b);
        self.register.call(&mut self.store, (p, b.len() as i32)).unwrap();
    }
    /// Blocks the calling thread until the run ends. The host decides which thread that is.
    pub fn run(&mut self, typed: bool, job: Job) -> GuestOut {
        let input = serde_json::to_vec(&GuestIn { typed_imports: typed, job }).unwrap();
        self.store.data().meter.cross("export:guest_run", input.len());
        let p = self.put(&input);
        let n = self.run.call(&mut self.store, (p, input.len() as i32)).expect("guest trapped");
        let op = self.out_ptr.call(&mut self.store, ()).unwrap();
        let mut out = vec![0u8; n as usize];
        self.memory.read(&self.store, op as usize, &mut out).unwrap();
        self.store.data().meter.cross("export:read_out", out.len());
        serde_json::from_slice(&out).unwrap()
    }
}
