//! PROTOTYPE (throwaway, ticket 36): the callback boundary, three ways.
//!
//! The pure core (State, transitions, `CancelToken`, `abortable`, `join_all`) is reused from
//! `prototypes/single-call-core/call-core`. This crate adds only what the three axes vary:
//!   `link`      `Link`: what a Loop needs from the host's callbacks, boundary-neutral. One loop
//!               (`drive`) and one Exchange stand-in run over any `Link`.
//!   `b1`        B1 + R1 + K2: closures with handles; the capability set as a value (`Caps1`);
//!               async and blocking registration forms; the blocking-runner capability.
//!   `b2`        B2 (and the library half of B3): everything as JSON bytes, handles as plain
//!               callbacks; `Wire` is the byte-level boundary, `Link2` the library's use of it.
//!   `context`   R2: a context object the library holds between calls.
//!   `abi`       the job and result types a Wasm guest takes and returns.
//! `no_std` + `alloc`, no threads, no unsafe.
#![no_std]
#![forbid(unsafe_code)]
extern crate alloc;

pub mod abi;
pub mod b1;
pub mod b2;
pub mod context;
pub mod link;

pub use call_core::*;
pub use link::*;
