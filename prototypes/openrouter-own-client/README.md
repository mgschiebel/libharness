# PROTOTYPE (throwaway): OpenRouter adapter on our own small client

Ticket: mgschiebel/libharness#33 ("Prototype: OpenRouter adapter on our own small client"). Branch `prototype/openrouter-own-client`, never merged to main.
Built on the codec and assembler of the provider-abstraction prototype (our own code, reused), extended with a real HTTP transport.

**Question:** what does the optional OpenRouter adapter cost end to end when we write our own small client: the no_std codec plus a real HTTP transport, compared across transports on dependency weight, runtime coupling and cancellation?

**NOT exercised against the live API.** No key, no network beyond a loopback mock. Every fixture is hand-built from the docs and OpenRouter's OpenAPI schemas (`ChatStreamChunk`, `ChatStreamToolCall`, `ChatUsage`), so the results test my reading of the docs, not OpenRouter's behaviour. **TLS is untested**: the mock speaks plain `http://`, so TLS is sized from dependency numbers only. Concepts and public API/docs only: nothing was copied from ureq, reqwest or any other project; the only reused code is our own provider-abstraction prototype.

## Run

```
cd prototypes/openrouter-own-client
cargo test                                   # 12 matrix tests (~15 s), 13 codec tests, 2 thread-cost tests
cargo run -q -p conformance --bin matrix     # prints the table below plus one detail line per cell
./target/debug/matrix threads                # 64-session thread-cost runs (TN=std EX=LocalPool narrows it)
./measure.sh                                 # dependency counts per transport variant
./build-times.sh                             # cold debug build per variant, fresh target dir each
cargo check -p provider-codec --target wasm32v1-none   # the codec is still no_std + alloc: passes
```

## Layout

| Crate | What | Std? |
|---|---|---|
| `provider-codec` | message/reply/error types, SSE parser, stream assembler, OpenRouter codec. Dep: `serde_json` (alloc) | `no_std` + `alloc`, passes `wasm32v1-none` |
| `provider-adapter` | the one transport seam (`Transport`, `Body`), `complete()`, the session-loop callback `model_callback()`, retry with a host-supplied `Sleep`, and `bridge` (blocking-thread-to-future channel, no runtime types) | std only, no tokio |
| `transport-ureq` | (a) `ureq` 3, one worker thread per call | std |
| `transport-reqwest` | (b) `reqwest` 0.13 + `stream`; optional `owned-runtime` workaround | tokio |
| `transport-std` | (c) hand-written HTTP/1.1 on `std::net`, `http://` only: the "host supplies it" reference | std only |
| `mock-server` | loopback mock on std sockets: real HTTP/1.1, chunked SSE, Content-Length errors, hold/abort/pause steps, request recording, client-close detection | std only |
| `conformance` | cases 1 to 8 plus five extras, three executors, 64-session thread test | |

The seam (unchanged in shape from the prototype except the lifetime, see Surprises):

```rust
pub trait Transport: Send + Sync { fn send(&self, req: HttpRequest) -> BoxFut<'static, Result<(ResponseHead, Box<dyn Body>), String>>; }
pub trait Body: Send { fn next_chunk(&mut self) -> BoxFut<'_, Option<Result<Vec<u8>, String>>>; } // dropping the Body closes the connection
```

## Answer

Writing our own client is cheap in code and the cost is almost entirely in the transport choice. The codec grew by 52 lines (574 to 626) to carry `reasoning`, `reasoning_details`, `cost_details`, `is_byok` and token details, and stays `no_std` + `alloc`. A transport is 38 to 99 lines of glue. All eight protocol cases pass over real loopback HTTP/1.1 with chunked SSE through the std-socket transport, and through `ureq` after one workaround (agent per call, for cancellation); `reqwest` passes only inside a tokio runtime and panics outside one, unless the adapter owns a runtime (another 43 lines and one more panic to defuse). Dependency weight is the real spread: 5 external packages (host supplies the transport), 14 (ureq, no TLS) or 26 (ureq + rustls/ring), 73 (reqwest, no TLS) or 86 (reqwest + rustls/aws-lc). The blocking transports cost one OS thread per in-flight call (64 sessions: 64 threads); reqwest costs none. The callback's `Err(String)` still cannot carry a retry hint, partial output, status or metadata, and no transport can wait before a retry without a host-supplied timer.

## Results: cases 1 to 8 (shared protocol) over real loopback HTTP/1.1, chunked SSE

Each cell is the case run on every executor (futures LocalPool, futures ThreadPool, tokio multi-thread) unless noted. Chunk sizes 1, 7 and 4096 bytes are used where a case parses text (HTTP chunk edges fall mid-line and mid-UTF-8).

| # | Case | (a) ureq 3, shared agent (naive) | (a) ureq 3, agent per call | (b) reqwest 0.13 in tokio | (b) reqwest, no tokio entered | (b) reqwest + owned runtime | (c) std sockets |
|---|---|---|---|---|---|---|---|
| 1 | SSE comments, also between events | pass | pass | pass | **fail** (panic) | pass | pass |
| 2 | Error chunk mid-stream in a 200, partial text kept | **fail** (socket stays open) | **workaround** | pass | fail (panic) | pass | pass |
| 3 | 2 parallel tool calls, split mid-token, interleaved | pass | pass | pass | fail (panic) | pass | pass |
| 4 | Usage with `cost`, `cost_details`, `is_byok`, duplicate `finish_reason` | pass | pass | pass | fail (panic) | pass | pass |
| 5 | Served `model` differs from requested | pass | pass | pass | fail (panic) | pass | pass |
| 6 | Dropping the call future closes the server-side socket | **fail** (open 3 s later) | **workaround** | pass | fail (panic) | **workaround** (abort the task) | pass (cancel hook) |
| 7 | `models`, `provider`, `reasoning`, `plugins` on the wire | pass (unchecked JSON) | pass | pass | fail (panic) | pass | pass |
| 8 | HTTP 429 + `Retry-After`, 402, 503 plain responses | pass | pass | pass | fail (panic) | pass | pass |

Notes per case:

- **1** passes at all three chunk sizes including CRLF events and a 4-byte emoji split across chunks.
- **2** partial text survives as `ProviderError.partial`; kind `RateLimited`, code 429, `metadata.error_type` kept. The cell is judged on two things: the partial, and the client hanging up after the error chunk instead of draining the stream (the mock keeps the stream open). `ureq` with a shared agent recovers the partial but never hangs up, because its worker is blocked in a read.
- **6** needs more than "drop the future": a blocking worker is parked in `read()`, so drop must also interrupt the socket. (c) registers `TcpStream::shutdown` as the cancel hook. (a) has no per-request cancel in ureq 3; `Agent::shutdown()` is agent-wide, so the working version builds one `Agent` per call and shuts that one down (about 8 extra lines, no connection pooling across calls). (b) needs nothing: dropping the response stream closes the connection (11 ms in the run).
- **7** all four fields reach the wire, as unchecked JSON in `ModelRequest.extra` and per-integration `body_defaults`. A typo key (`provder`) also reaches the wire. `extra` is merged after the typed fields, so `{"stream": false, "model": "evil/model"}` silently overrides them (asserted, not fixed). Nothing is typed.
- **8** 429 gives `RateLimited`, `retry_after_secs = 7`, retryable; 402 gives `Credits`, not retryable; 503 with an HTML body gives `Upstream` with status 503. `ureq` returns 4xx/5xx as `Err` by default; `http_status_as_error(false)` is required to see OpenRouter's JSON error body.

Extra checks run through the same matrix (all pass on (a) final, (c) and (b) in tokio or on an owned runtime):

| # | Check | Result |
|---|---|---|
| X1 | `delta.reasoning` text, `reasoning_details` fragments merged by `index`, echoed in the next request | pass |
| X2 | incremental delivery: first text delta seen vs end of stream, server pauses 500 ms mid-stream | first delta at 0 to 2 ms, stream end at ~501 ms, on every transport |
| X3 | connection cut mid-body (no terminating chunk) and connection refused | `Transport` error with partial `"abc"` kept; message text differs per transport, see error flattening |
| X4 | retry on 429 `Retry-After: 1` with a host-supplied sleeper | 2 attempts, waited ~1005 ms, on every transport |
| X5 | what `Err(String)` of the callback keeps | see error flattening |

## Runtime test: futures-executor with no tokio, and inside tokio

All 13 checks per cell. "No tokio" means the test never enters a tokio runtime; the adapter crates for (a) and (c) have no tokio in `cargo tree`.

| Transport | futures `LocalPool` | futures `ThreadPool` | tokio multi-thread |
|---|---|---|---|
| (a) ureq, agent per call | 13/13 | 13/13 | 13/13 |
| (c) std sockets | 13/13 | 13/13 | 13/13 |
| (b) reqwest | **0/13: panic** | **0/13: panic** | 13/13 |
| (b) reqwest + owned runtime | 13/13 | 13/13 | 13/13 |

What fails without tokio for reqwest: not an error value but a panic, `there is no reactor running, must be called from the context of a Tokio 1.x runtime`, raised from `tokio::net::TcpStream` registration on the first poll. On `LocalPool` it unwinds out of `run_until`. On `ThreadPool` it kills the pool worker thread and the task silently vanishes (the test saw the result channel disconnect; a host without that probe would see a session that never finishes, and a pool of N workers is gone after N such calls). The mock URL is an IP literal; with a hostname the panic would come earlier (DNS runs on tokio's blocking pool), untested.

The owned-runtime workaround: the adapter holds a one-thread tokio runtime, spawns the request on it, and hands the host a runtime-neutral bridge future (the same `bridge` the blocking transports use, plus `Sender::send_nowait` so a runtime thread never parks). It passes everywhere but is the adapter owning a tokio runtime, which is what "no tokio in the adapter" ruled out. Its first version panicked in every case: the transport (and so the runtime) is dropped on whatever thread the host drops it, and dropping a tokio `Runtime` inside another runtime panics (`Cannot drop a runtime in a context where blocking is not allowed`). The fix is a `Drop` that calls `shutdown_background()`.

## 64 parallel sessions (thread cost of a blocking transport)

64 sessions started at once through `model_callback`. The mock holds every stream open until all 64 are connected, then 300 ms more. Each configuration ran in its own process; threads are counted from `/proc/self/task` by name, RSS from `VmRSS`. All 64 sessions returned the right text in every run.

| Transport | Executor | Worker threads at peak | Other threads (runtime/executor) | RSS delta |
|---|---|---|---|---|
| (c) std sockets | LocalPool (1 thread) | **64** | 0 | +3.8 MB |
| (c) std sockets | ThreadPool (2) | 64 | 2 | +4.2 MB |
| (c) std sockets | tokio | 64 | 2 | +3.9 MB |
| (a) ureq | LocalPool | **64** | 0 | +24 MB |
| (a) ureq | ThreadPool | 64 | 2 | +24 MB |
| (a) ureq | tokio | 64 | 2 | +24 MB |
| (b) reqwest | tokio (2 workers) | **0** | 2 (tokio's) | +4.4 MB |
| (b) reqwest + owned runtime | LocalPool | 0 | 1 (`lh-reqwest-rt`) | +4.3 MB |

A blocking transport costs exactly one OS thread per in-flight call (spawned lazily at first poll, gone when the body ends or is dropped), independent of the executor: even a single-threaded `LocalPool` has 64 worker threads behind it. Each thread reserves a 2 MiB stack by default (about 128 MiB virtual for 64; not measured as resident). The threads live as long as the generation, seconds to minutes, so this scales with concurrent sessions, not with request rate. ureq's extra ~20 MB resident is about 375 KB per session against about 60 KB for the std transport; ureq's `Config` has 128 KiB input and output buffers (read in its source; I did not isolate the cause). The "other threads" column excludes the sampler, the main thread and threads in the first microseconds of their life before they are named. The mock server's own 64 connection threads are in the same process and are reported separately by the tool, not counted here.

## Measurements per transport

Machine note: 2 cores, and other builds shared it. `/proc/loadavg` read 0.86 before the first build and 1.95 after the last, so **all timings are noisy**; user+sys CPU seconds are the steadier figure. Registry already downloaded. Dependency numbers are from `cargo tree -e normal` for the host target plus `cargo metadata` (build.rs = a `custom-build` target), counting unique external packages (the workspace's own crates excluded, the transport crate excluded). The weights are for the transport crate with the adapter and codec underneath.

| Variant | External packages | Packages with build.rs | Cold debug build, wall | user + sys CPU |
|---|---|---|---|---|
| adapter + codec only (host supplies the transport) | 5 | 3 (serde_core, serde_json, zmij) | 3.7 s | 5.5 s |
| (c) std sockets | 5 | 3 | 4.0 s | 5.8 s |
| (a) ureq 3, no TLS | 14 | 4 (+httparse) | 6.1 s | 10.7 s |
| (a) ureq 3, rustls + ring (default) | 26 | 7 (+libc, ring, rustls) | 14.0 s | 25.2 s |
| (b) reqwest 0.13 + `stream`, no TLS | 73 | 9 (+icu_normalizer_data, icu_properties_data, libc, proc-macro2, quote) | 18.4 s | 34.1 s |
| (b) reqwest 0.13 + `stream`, rustls (default, aws-lc) | 86 | 12 (+aws-lc-rs, aws-lc-sys, rustls) | 44.8 s | 81.4 s |
| (b) + `owned-runtime` feature | 86 (tokio is already in the tree) | 12 | not rebuilt | |

TLS is therefore sized, not tested: it adds 12 packages and 3 build scripts to ureq (6.1 s to 14.0 s) and 13 packages and 3 build scripts to reqwest (18.4 s to 44.8 s, `aws-lc-sys` compiles C). These match the earlier research (ureq default 27, reqwest rustls 87 including its probe root). With the host-supplied transport the TLS, proxy and DNS policy are the host's and the adapter stays at 5 packages. The licence picture for the TLS variants (ISC, Zlib, CDLA-Permissive-2.0 and others) is as recorded in the research and was not re-run. The 5-package figure corrects the provider-abstraction README, which said 4: it left out `serde_json` itself (serde_json, serde_core, itoa, memchr, zmij).

### Adapter glue per transport (lines, excluding tests, fixtures and the mock)

| Piece | Total lines | Code lines (no blank/comment) |
|---|---|---|
| shared: seam + `complete()` + `model_callback` (in `provider-adapter/src/lib.rs`, first 81 lines) | 81 | about 60 |
| shared: retry with a host `Sleep` (`complete_with_retry`, `model_callback_retrying`) | 35 | about 32 |
| shared by (a), (c) and the (b) workaround: `bridge.rs` (channel, cancel hook, `spawn_call`, `attach`, plus a 25-line `thread_sleep` timer for the retry test) | 185 | 159 |
| (a) `transport-ureq` (incl. the naive constructor and the cancel workaround, about 12 lines) | 64 | 56 |
| (c) `transport-std` | 99 | 92 |
| (b) `transport-reqwest`, plain (no bridge, no thread) | about 38 | about 34 |
| (b) owned-runtime workaround on top (struct, `Drop`, spawn, bridge hookup) | about 43 | about 40 |

So the per-transport cost is 64 lines for ureq, 99 for std sockets, 38 for reqwest (43 more for the runtime-neutral variant), each of the blocking ones also leaning on the 185-line `bridge`. For scale, the prototype's driver, callback and seam were 75 lines.

### Codec size after adding the dropped fields

Counted as total lines of `provider-codec/src/*.rs` (the stub integration was dropped from this prototype: it was only there to test the trait and is not part of the OpenRouter cost).

| Step | Lines | Change | Cheap? |
|---|---|---|---|
| baseline (prototype minus stub) | 574 | | |
| `reasoning` (`delta.reasoning` to `StreamEvent::Reasoning`, `ModelReply.reasoning`) | 585 | +11, 3 files, no public break except one new field | yes |
| `reasoning_details`: collect, merge fragments by `index` and `type`, echo on `Message::Assistant` | 616 | +31, 3 files | not trivial: needs a merge rule (text/summary concatenate, other keys overwrite, nulls ignored) that is **my assumption** about how fragments arrive, and the `Message::Assistant` variant gained a field, a breaking change for every constructor of it (one test line here) |
| `cost_details` (kept as opaque JSON), `is_byok`, `reasoning_tokens`, `cached_tokens` | 626 | +10, 2 files | yes |
| served `model` | 626 | 0 | already in the prototype: the `Model` event and `ModelReply.model` existed; the ticket's list of dropped fields was wrong on this one |
| `cost` | 626 | 0 | already typed `Option<f64>`; the unit (credits or USD) is still unstated by the schema |

Total 574 to 626 (+52, +9%). Still `#![no_std]` + `alloc` with `serde_json` (alloc) as the only dependency; `cargo check -p provider-codec --target wasm32v1-none` passes after every step. Not typed on purpose: `cost_details` and each `reasoning_details` entry stay `serde_json::Value`.

### Error-type flattening

- **Callback boundary.** `model_callback` returns `Result<ModelReply, String>` (the session loop's shape). `ProviderError` has `kind` (8 values), `status`, `code`, `retry_after_secs`, `metadata` (JSON) and `partial` (the assembled reply); `.to_string()` keeps only `"{kind:?}: {message}"`. X5 shows it end to end: a 429 with `Retry-After: 7` becomes `"RateLimited: slow down"`; a mid-stream error after `"partial "` becomes `"RateLimited: Rate limit exceeded"`. Retry hint, status, error_type metadata and the partial text are all gone, so a caller of the callback can neither back off correctly nor recover the text already streamed. `complete()` returns the structured error; only the callback flattens it.
- **Transport seam.** Each transport flattens its own error type to `String` at the seam, and the adapter maps every one to `ErrorKind::Transport`. Observed messages for the same two faults (cut body, refused connection):

| Transport | cut body | connection refused |
|---|---|---|
| (c) std | `connection closed before the terminating chunk` (my own text) | `connect 127.0.0.1:PORT: Connection refused (os error 111)` |
| (a) ureq | `Peer disconnected` | `io: Connection refused (os error 111)` |
| (b) reqwest | `error decoding response body` | `error sending request for url (http://127.0.0.1:PORT/api/v1/chat/completions)` |

  reqwest's `Display` shows only the outermost message: the cause (connection refused) is in `source()` and is lost by `to_string()`. None of the three says whether the fault is a timeout, a connect failure, a TLS failure or a reset, so the adapter cannot decide retryability from the transport error beyond "it is a transport error". A structured transport error (kind plus retryable flag) at the seam would be needed, which is the same gap as the callback's.

### The retry-without-a-timer problem

The library has no timers (ADR 0002), so waiting for `Retry-After` has no home in the core. What the prototype shows: `complete_with_retry` takes a host-supplied `Sleep` (`Fn(Duration) -> BoxFut<()>`) and retries only when the error is retryable and nothing was delivered yet (otherwise a retry would duplicate the partial text); it is about 35 lines and passes X4 on all three transports and all three executors. With no host timer, the adapter's only runtime-neutral option is the one in `bridge::thread_sleep`: one sleeping OS thread per wait (cancelled by drop only after it wakes). Per transport: the blocking ones could instead sleep inside their worker thread, which is invisible to the future but cannot be interrupted by a drop except by waking the worker; reqwest can use `tokio::time::sleep`, but only inside tokio (and the owned-runtime variant would need its own). No jitter either: the library has no randomness. And retry inside the adapter cannot be undone by the session loop, while retry outside it is impossible with `Err(String)`. So the options are a host timer capability, or a structured error the session loop can act on, or retry in the transport thread.

## Surprises

1. **The `Body` seam from prototype #7 could not be implemented by a body that owns a stream.** `next_chunk` returned a `'static` future, so any body holding a reqwest stream or a channel receiver would need `Arc<Mutex<_>>` around it. The fixtures hid it because the fake body owned nothing. Fixed by lending the body to the future (`BoxFut<'_, _>`, a 2-word change); the lesson is that the seam shape was only validated by a real transport.
2. **ureq 3 has no per-request cancel, and a dropped call leaves its worker parked in `read()`.** Cases 2 and 6 fail with the obvious build (the socket stays open until the server speaks or closes, here 30 s). The documented escape is `Agent::shutdown()`, which is agent-wide and refuses later requests, so the working build is one `Agent` per call (about 8 lines), giving up connection pooling and its TLS session reuse.
3. **reqwest outside tokio is a panic, not an error, and on a futures `ThreadPool` it silently kills the worker.** Nothing in the seam can turn that into an `Err`. The owned-runtime escape then had its own panic on drop (a `Runtime` dropped inside another runtime).
4. **Two of the four "dropped fields" were not dropped.** The served model and `cost` were already in the prototype; the real gaps were `reasoning`, `reasoning_details` and `cost_details`/`is_byok`. Only `reasoning_details` was more than a few lines, and it is the only one resting on an unobserved assumption (how fragments of one block arrive).
5. **`extra` can silently overwrite `stream`, `model` and `messages`.** Case 7's "unchecked JSON side channel" is worse than a typo hazard: a typed key can be clobbered by the escape hatch. Not fixed here.
6. **reqwest's error `Display` drops the cause**, so the flattened `String` for "connection refused" is just "error sending request for url (...)".
7. **The 5-vs-4 package count**: the earlier README undercounted the codec's dependencies by one (it skipped `serde_json` itself).

Things that merely confirm expectations:

- Incremental SSE delivery works through all three transports (first delta at 0 to 2 ms against a stream that ends at about 500 ms), including HTTP chunk edges inside SSE lines and inside UTF-8 characters.
- Drop-to-close works for the std transport with a socket shutdown hook and for reqwest by plain drop.
- The seam stays runtime-neutral for (a) and (c): no tokio anywhere in their trees; the same futures run unchanged on `LocalPool`, `ThreadPool` and inside tokio.
- The codec stays `no_std` + `alloc` after the additions; only the transports need std.
- A blocking transport costs one thread per in-flight call, as inferred in the research; 64 sessions work with 64 threads.
- Dependency weight is dominated by the TLS and HTTP stack choice, not by the adapter: 5 packages without a transport, 86 with reqwest and rustls.

## What this does not tell you

TLS, proxies, redirects and gzip are untested (the mock is plain HTTP). Real OpenRouter fragmenting of tool calls and reasoning blocks is unobserved. Build times are single noisy runs. The 64-session figures come from one process per cell and one run each. HTTP keep-alive and connection reuse were not exercised (the mock sends `Connection: close`). Cost units are still unknown.
