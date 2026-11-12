//! PROTOTYPE (throwaway): the OpenRouter adapter built on the `openrouter-rs` crate.
pub mod adapter;
pub mod model;
#[cfg(feature = "private-runtime")]
pub mod private_runtime;

pub use adapter::{model_callback, BoxFut, OpenRouterAdapter};
pub use model::*;
