//! Probe cdylib: exercises the public API so --gc-sections keeps (and the import check sees) its code.
#![no_std]
extern crate alloc;
use nostd_core::{demo, schema_of, VecStream};
use nostd_core::Stream;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { core::arch::wasm32::unreachable() }

// Global allocator for the cdylib: tiny bump allocator over linear memory (no imports).
mod bump {
    use core::alloc::{GlobalAlloc, Layout};
    use core::sync::atomic::{AtomicUsize, Ordering};
    pub struct Bump;
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    unsafe impl GlobalAlloc for Bump {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            let size = l.size().max(1);
            let mut cur = NEXT.load(Ordering::Relaxed);
            if cur == 0 { cur = (core::arch::wasm32::memory_size(0) * 65536) / 2; }
            let p = (cur + l.align() - 1) & !(l.align() - 1);
            NEXT.store(p + size, Ordering::Relaxed);
            while p + size > core::arch::wasm32::memory_size(0) * 65536 { if core::arch::wasm32::memory_grow(0, 4) == usize::MAX { return core::ptr::null_mut(); } }
            p as *mut u8
        }
        unsafe fn dealloc(&self, _: *mut u8, _: Layout) {}
    }
}
#[global_allocator]
static A: bump::Bump = bump::Bump;

#[no_mangle]
pub extern "C" fn run() -> i64 {
    let caps = demo::caps();
    let _ = (caps.tool_specs().len(), caps.result_schema().is_object(), schema_of::<demo::Answer>());
    let mut s = VecStream(alloc::vec![1i64, 2]);
    let _ = core::pin::Pin::new(&mut s).size_hint();
    match demo::block_ready(caps.run_once("hi")) { Some(Ok(a)) => a.total + caps.calls_made() as i64 + caps.now_ms() as i64, _ => -1 }
}
