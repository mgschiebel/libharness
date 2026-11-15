//! PROTOTYPE (throwaway, ticket 36): the host's side. Executors, threads, sockets and
//! wasmtime appear here and nowhere in the gated crates.
pub mod host;
pub mod mock_net;
pub mod rig;
pub mod wasm;
pub mod wire;
pub mod scenarios;
