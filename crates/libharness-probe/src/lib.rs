//! Probe cdylib: exports `run`, which calls into every gated crate so link-time dead-code elimination
//! keeps (and the import check sees) their code. Known limit, documented in ADR 0002: code the probe
//! never calls can still hide an import.
//!
//! The probe is outside the gate, so it may use `unsafe` for the one thing a cdylib must bring: an
//! allocator. Everything above it stays under `forbid(unsafe_code)`.

#![no_std]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

/// A bump allocator over linear memory. It never frees and imports nothing.
struct Bump;

static NEXT: AtomicUsize = AtomicUsize::new(0);
const PAGE: usize = 65536;

unsafe impl GlobalAlloc for Bump {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut cur = NEXT.load(Ordering::Relaxed);
        if cur == 0 {
            // Start halfway through the initial memory, clear of the data and stack linked below it.
            cur = core::arch::wasm32::memory_size(0) * PAGE / 2;
        }
        let start = (cur + layout.align() - 1) & !(layout.align() - 1);
        let end = start + layout.size().max(1);
        while end > core::arch::wasm32::memory_size(0) * PAGE {
            if core::arch::wasm32::memory_grow(0, 4) == usize::MAX {
                return core::ptr::null_mut();
            }
        }
        NEXT.store(end, Ordering::Relaxed);
        start as *mut u8
    }

    unsafe fn dealloc(&self, _: *mut u8, _: Layout) {}
}

#[global_allocator]
static ALLOC: Bump = Bump;

/// Calls into the gated crates. Returns a number only so the calls cannot be optimised away.
#[unsafe(no_mangle)]
pub extern "C" fn run() -> i64 {
    let stub = libharness::Stub::new();
    let counter = libharness::Counter::default();
    stub.label().len() as i64 + counter.bump() as i64
}
