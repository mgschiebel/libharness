//! libharness core (stub).
//!
//! This crate is the pure, gated core of the library: `no_std` + `alloc`, no `unsafe`, no I/O.
//! The real State, transitions and capability builder arrive in later tickets (#40 onwards); today
//! the crate exists so that every no-I/O enforcement layer has something to check from the first commit.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use spin::Mutex;

/// The library's name, as hosts will see it in logs and errors.
pub const NAME: &str = "libharness";

/// Placeholder so the gate exercises both dependencies (`serde` and `spin`). Replaced by the real State.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Stub {
    label: String,
}

impl Stub {
    /// A stub value labelled with the library name.
    pub fn new() -> Self {
        Self {
            label: String::from(NAME),
        }
    }

    /// The label.
    pub fn label(&self) -> &str {
        &self.label
    }
}

impl Default for Stub {
    fn default() -> Self {
        Self::new()
    }
}

/// A counter behind a `spin` lock, the one lock the core is allowed. The clippy rule in `clippy.toml`
/// forbids holding its guard across an `.await`.
#[derive(Debug, Default)]
pub struct Counter(Mutex<u64>);

impl Counter {
    /// Add one and return the new value. The guard is dropped before returning.
    pub fn bump(&self) -> u64 {
        let mut guard = self.0.lock();
        *guard += 1;
        *guard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_carries_the_library_name() {
        assert_eq!(Stub::new().label(), NAME);
    }

    #[test]
    fn counter_counts() {
        let c = Counter::default();
        assert_eq!(c.bump(), 1);
        assert_eq!(c.bump(), 2);
    }
}
