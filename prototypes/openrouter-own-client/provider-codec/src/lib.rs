//! PROTOTYPE (throwaway). Pure, I/O-free half of a model-call adapter: the types the core
//! would see, the `Provider` trait (a codec: request in, bytes decoded to events), a shared
//! SSE line parser and a shared stream assembler. `no_std` + `alloc`, no runtime types.
#![no_std]
extern crate alloc;

pub mod assembler;
pub mod openrouter;
pub mod sse;
pub mod types;

pub use assembler::Assembler;
pub use types::*;

use alloc::vec::Vec;

/// The whole abstraction: 2 methods + 1 associated type (+ the 2-method `Decoder`).
pub trait Provider {
    type Decoder: Decoder;
    /// Encode a model request as an HTTP request. Pure; no I/O.
    fn request(&self, req: &ModelRequest) -> HttpRequest;
    /// Make a decoder for one response. Sees status and headers so it can switch to
    /// error-body mode (4xx/5xx) without a third trait method.
    fn decoder(&self, head: &ResponseHead) -> Self::Decoder;
}

/// Bytes in (any chunking), normalized events out.
pub trait Decoder {
    fn push(&mut self, bytes: &[u8], out: &mut Vec<StreamEvent>);
    /// The body ended (cleanly or not). Flush; a buffered integration emits everything here.
    fn finish(&mut self, out: &mut Vec<StreamEvent>);
}
