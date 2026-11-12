---
status: accepted
---

# No-I/O is enforced by a std-less compile gate

The core performs no I/O, and that is enforced by the build rather than by convention. Every crate in the core is `#![no_std]` + `alloc` and must pass `cargo check -p <crate> --target wasm32v1-none`, which fails if anything in its dependency tree needs `std`. A std denylist stack (clippy `disallowed_*`, `forbid`, cargo-deny) was red-teamed and still missed 9 of 29 escapes, so it is not built; it stays a documented fallback. Decided 2026-11-10 in the no-I/O enforcement grilling, starting from the recommendation in issue 24.

## Layers

All required in CI, on every gated crate:

- The `wasm32v1-none` per-package check, never workspace-wide, with `default-members` set so a plain `cargo build` works.
- `forbid(unsafe_code)`.
- A cargo-deny allowlist with `external-default-features = "deny"`, plus a separate check of each gated crate's own build script (cargo-deny does not apply its build-script allowance to workspace members).
- Clippy with `await_holding_invalid_type` set for `spin::MutexGuard`, so a `spin` lock is never held across an `.await`.
- A zero-import check on the wasm probe build. Dead-code elimination can hide imports in code the probe never calls; that is a known limit, not closed.

Each workspace crate declares `gated = true` or `gated = false` in its Cargo metadata. CI gates every `true` crate and fails on a crate that declares neither, so a new `no_std` crate cannot slip in ungated.

## Crate layout

- `libharness`: the main crate hosts depend on. Sessions, model types, capability registry. Gated.
- `libharness-codec`: the `Provider` and `Decoder` traits and one module per wire format (OpenRouter, Anthropic), with no cargo features, because features can unify and switch `std` on in the gated tree. Gated; depends on `libharness` for the model types.
- `libharness-openrouter`: the std adapter. The HTTP transport lives here. Outside the gate and exempt from the allowlist, but subject to the permissive-licence check from ADR 0001. Depends on both.
- `libharness-probe`: unpublished cdylib, `gated = false`, outside `default-members`. Depends on every gated crate and exports one function per gated crate that runs a canned session or decode against fixtures.

There is no facade crate. The CI toolchain is pinned in `rust-toolchain.toml` with the `wasm32v1-none` target; MSRV policy is left open.

## Capabilities the core needs from the host

The clock and id generation are ordinary host-registered capabilities, synchronous plain functions, both required to start a session, with no defaults (a silent default hides a missing clock). Narrowed by ADR 0003: the pure core takes no clock, and the clock is required only where something needs it. There is no randomness capability: nothing in the core needs it, and retry jitter belongs to the host because the library has no timers.

## Callbacks stay serialisable

Every value crossing a callback is owned, serialisable data: no borrowed lifetimes, generics or trait objects in requests, replies, stream events, tool arguments and results, typed results or errors. Only the callbacks themselves are Rust closures. A Wasm component runtime as a true sandbox is not in the first release and stays open, but this keeps it possible: a crate that builds for `wasm32v1-none` with zero imports is already a valid guest.

## Accepted costs

`hashbrown` instead of std `HashMap`; `spin` instead of std `Mutex`; time only through the injected clock; about 27 external crates in the core.

## Left open

Who reviews changes to the gate configuration is repository maintenance, not library design, and is out of scope. The Wasm component runtime, MSRV policy and the structured, serialisable error type for the model-call callback (grilling issues 8 and 9) are open.
