//! PROTOTYPE (B3): the core's Loop and Exchange as a Wasm guest (wasm32v1-none, no WASI, no
//! threads). Callbacks are host imports. The guest is single-threaded and synchronous: an
//! async operation is `start` (returns an id at once), `poll`, `take`, and when nothing can
//! progress the guest's tiny `block_on` calls the `wait` import, which blocks this one thread.
//! All the library logic is `cb_core`; everything here is glue (imports, allocator, exports).
#![no_std]
extern crate alloc;

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use cb_core::abi::*;
use cb_core::b2::*;
use cb_core::*;
use core::future::Future;
use core::pin::{pin, Pin};
use core::task::{Context, Poll, Waker};
use spin::Mutex;

#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

#[link(wasm_import_module = "h")]
extern "C" {
    // generic form: one import for every operation kind
    fn call_start(name: *const u8, name_len: usize, args: *const u8, args_len: usize) -> i32;
    // typed form: one import per operation kind
    fn model_start(args: *const u8, len: usize) -> i32;
    fn tool_start(args: *const u8, len: usize) -> i32;
    fn transport_start(args: *const u8, len: usize) -> i32;
    fn body_next_start(args: *const u8, len: usize) -> i32;
    // shared
    fn op_poll(op: i32) -> i32;
    fn op_take(op: i32, out: *mut u8, cap: usize) -> i32;
    fn op_drop(op: i32);
    fn wait();
    fn cancelled() -> i32;
    fn emit(ev: *const u8, len: usize);
    fn validate(v: *const u8, len: usize, err: *mut u8, cap: usize) -> i32;
    fn inbox(out: *mut u8, cap: usize) -> i32;
    fn body_close(handle: i64);
}

struct WasmWire {
    typed: bool,
}
struct OpFut {
    id: i32,
    done: bool,
}
impl Future for OpFut {
    type Output = Result<Vec<u8>, String>;
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        let n = unsafe { op_poll(self.id) };
        if n < 0 {
            return Poll::Pending;
        }
        let mut buf = vec![0u8; n as usize];
        unsafe { op_take(self.id, buf.as_mut_ptr(), buf.len()) };
        self.done = true;
        Poll::Ready(if buf[0] == 0 { Ok(buf[1..].to_vec()) } else { Err(String::from_utf8_lossy(&buf[1..]).to_string()) })
    }
}
impl Drop for OpFut {
    fn drop(&mut self) {
        if !self.done {
            unsafe { op_drop(self.id) } // dropping a pending call cancels the host's future
        }
    }
}

impl Wire for WasmWire {
    fn call(&self, op: &str, args: Vec<u8>, _plain: Plain) -> BoxFut<Result<Vec<u8>, String>> {
        let (a, l) = (args.as_ptr(), args.len());
        let id = unsafe {
            if !self.typed {
                call_start(op.as_ptr(), op.len(), a, l)
            } else {
                match op {
                    "model" => model_start(a, l),
                    "tool" => tool_start(a, l),
                    "transport" => transport_start(a, l),
                    _ => body_next_start(a, l),
                }
            }
        };
        Box::pin(OpFut { id, done: false })
    }
    fn validate(&self, v: &[u8]) -> Result<(), String> {
        let mut err = [0u8; 256];
        let n = unsafe { validate(v.as_ptr(), v.len(), err.as_mut_ptr(), err.len()) };
        if n == 0 { Ok(()) } else { Err(String::from_utf8_lossy(&err[..n as usize]).to_string()) }
    }
    fn inbox(&self) -> Vec<u8> {
        let mut out = vec![0u8; 4096];
        let n = unsafe { inbox(out.as_mut_ptr(), out.len()) };
        out.truncate(n as usize);
        out
    }
    fn emit(&self, ev: &[u8]) {
        unsafe { emit(ev.as_ptr(), ev.len()) }
    }
    fn close(&self, _kind: &str, handle: u64) {
        unsafe { body_close(handle as i64) }
    }
}

/// Poll the root future; when it cannot progress, block in `wait` (the host returns when any
/// operation completes, the host asked for abort, or a short poll interval passed).
fn block_on<F: Future>(f: F, tok: &CancelToken) -> F::Output {
    let mut f = pin!(f);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if unsafe { cancelled() } != 0 {
            tok.cancel();
        }
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        unsafe { wait() };
    }
}

static HELD: Mutex<Option<Spec>> = Mutex::new(None);
static OUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

#[no_mangle]
pub extern "C" fn guest_alloc(n: i32) -> i32 {
    let mut v = Vec::<u8>::with_capacity(n as usize);
    let p = v.as_mut_ptr();
    core::mem::forget(v);
    p as i32
}
/// R2 inside Wasm: the instance holds the registrations between runs.
#[no_mangle]
pub extern "C" fn guest_register(ptr: i32, len: i32) {
    let b = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    *HELD.lock() = Some(serde_json::from_slice(b).unwrap());
}
#[no_mangle]
pub extern "C" fn guest_run(ptr: i32, len: i32) -> i32 {
    let b = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    let gi: GuestIn = serde_json::from_slice(b).unwrap();
    let tok = CancelToken::new();
    let link = Link2 { wire: Arc::new(WasmWire { typed: gi.typed_imports }), token: tok.clone() };
    let held = HELD.lock().clone();
    let out = block_on(run_job(&link, gi.job, held), &tok);
    let bytes = serde_json::to_vec(&out).unwrap();
    let n = bytes.len() as i32;
    *OUT.lock() = bytes;
    n
}
#[no_mangle]
pub extern "C" fn guest_out_ptr() -> i32 {
    OUT.lock().as_ptr() as i32
}
