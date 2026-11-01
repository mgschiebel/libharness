# Sans-IO design in Rust: patterns, host driving, pitfalls

Ticket: mgschiebel/libharness#4. Researched 2026-11-01 from primary sources only. Links to `main` were read on that date and may drift. rustls links use the `v/0.23.31` tag.

Convention: **[Source]** statements are backed by the linked source. **[Inference]** statements are my own reasoning for an agent harness. No primary source covers agent harnesses, so those are not sourced. **[Unknown]** marks what I did not verify.

## 1. The principle

- A sans-IO implementation <q>contains no code that does any form of network I/O or any form of asynchronous flow control</q>. It is <q>defined entirely in terms of synchronous functions returning synchronous results, and that does not block or wait for any form of I/O</q>. [Source: sans-io.readthedocs.io, "Writing I/O-Free Protocol Implementations"](https://sans-io.readthedocs.io/how-to-sans-io.html)
- Stated goal is reusability. Leaving I/O out lets one library serve both sync and async callers, and lets users <q>drive the network interactions themselves</q>. This differs from merely abstracting I/O. [Source: sans-io landing page](https://sans-io.readthedocs.io/)
- Claimed benefits are simplicity, testability and correctness. Tests push bytes in and check bytes out, with no sockets and no flaky tests. [Source: how-to-sans-io, "Simplicity, Testability, and Correctness"](https://sans-io.readthedocs.io/how-to-sans-io.html)
- The docs relate it to Clean Architecture and to "Functional Core and Imperative Shell". [Source: how-to-sans-io, Note](https://sans-io.readthedocs.io/how-to-sans-io.html)
- Note: the principle docs are Python-oriented. Everything Rust-specific below comes from crate docs and source.

## 2. Core shape: a state machine with inputs, outputs, and time as input

Three Rust crates, three variations of the same shape.

### quinn-proto (QUIC)

- <q>Fully deterministic</q>. It has <q>no networking code and does not get any relevant timestamps from the operating system</q>. [Source: quinn-proto lib.rs](https://github.com/quinn-rs/quinn/blob/main/quinn-proto/src/lib.rs)
- `Endpoint` dispatches datagrams. `Connection` holds the per-connection logic. [Source: same](https://github.com/quinn-rs/quinn/blob/main/quinn-proto/src/lib.rs)
- Input methods take `now: Instant` as an argument:
  - `Endpoint::handle(now, remote, local_ip, ecn, data, buf)` [Source: endpoint.rs](https://github.com/quinn-rs/quinn/blob/main/quinn-proto/src/endpoint.rs)
  - `Connection::handle_event(ConnectionEvent)`
  - `Connection::handle_timeout(now)`
- Output is pulled with `poll_*` methods:
  - `poll_transmit(now, max_datagrams, buf)`
  - `poll()` for application events
  - `poll_endpoint_events()`
  - `poll_timeout() -> Option<Instant>`
  - [Source: connection/mod.rs](https://github.com/quinn-rs/quinn/blob/main/quinn-proto/src/connection/mod.rs)
- The docs say when to poll. Poll for transmit after I/O on the connection, after `handle_event`, and after `handle_timeout`. Poll for events after `handle_event` and `handle_timeout`. [Source: connection/mod.rs doc comments](https://github.com/quinn-rs/quinn/blob/main/quinn-proto/src/connection/mod.rs)
- `handle_timeout`: <q>spurious extra calls will simply no-op and therefore are safe</q>. [Source: same](https://github.com/quinn-rs/quinn/blob/main/quinn-proto/src/connection/mod.rs)
- The crate is positioned for use <q>from a C or C++ project through C bindings</q> or <q>a different event loop than the one tokio provides</q>. [Source: lib.rs](https://github.com/quinn-rs/quinn/blob/main/quinn-proto/src/lib.rs)

### str0m (WebRTC)

- <q>The `Rtc` instance itself is not doing any network talking. Furthermore it has no internal threads or async tasks.</q> [Source: str0m README](https://github.com/algesten/str0m/blob/main/README.md)
- <q>Sans I/O is a pattern where we turn both network input/output as well as time passing into external input to the API</q>, giving <q>an enormous state machine that is driven forward by different kinds of input</q>. [Source: same](https://github.com/algesten/str0m/blob/main/README.md)
- It <q>never calls `Instant::now()` itself</q>. As a result you can drive tests faster than real time. [Source: README, "Now"](https://github.com/algesten/str0m/blob/main/README.md)
- API: `handle_input(Input)`, where `Input` is `Receive` or `Timeout`. Output comes from `poll_output()`, which returns `Transmit`, `Event` or `Timeout(next_deadline)`. [Source: README, "Run loop"](https://github.com/algesten/str0m/blob/main/README.md)

### rustls (TLS), unbuffered API

- It is an alternative to the `ConnectionCommon` API. It <q>does not internally buffer TLS nor plaintext data</q>. The caller owns the buffers. [Source: rustls lib.rs, `unbuffered` docs](https://github.com/rustls/rustls/blob/v/0.23.31/rustls/src/lib.rs)
- <q>Built around a state machine interface where the API user must handle each state to advance</q>. <q>No IO happens internally.</q> It also avoids `std::io::Read`/`Write`, so it is usable in no-std. [Source: same](https://github.com/rustls/rustls/blob/v/0.23.31/rustls/src/lib.rs)
- Entry point: `process_tls_records(incoming, outgoing)`. It returns an `UnbufferedStatus { discard, state }`. [Source: conn/unbuffered.rs](https://github.com/rustls/rustls/blob/v/0.23.31/rustls/src/conn/unbuffered.rs)
- `ConnectionState` is a `#[non_exhaustive]` enum: `ReadTraffic`, `EncodeTlsData`, `TransmitTlsData`, `BlockedHandshake`, `WriteTraffic`, `PeerClosed`, `Closed`, and others. Each variant tells the host what to do next. [Source: same](https://github.com/rustls/rustls/blob/v/0.23.31/rustls/src/conn/unbuffered.rs)

Two output styles recur.
- **Pull-style `poll_*` outputs** (quinn-proto, str0m): the core queues work and the host drains it.
- **State-enum return** (rustls unbuffered): each call returns "what to do next" as a typed state.
- The principle docs cover the same split. Output can go to an internal buffer, as in hyper-h2, or be returned directly, as in h11. A buffer is recommended when receiving input can itself produce output. [Source: how-to-sans-io, "Inputs and Outputs"](https://sans-io.readthedocs.io/how-to-sans-io.html)
- The principle docs also describe events as the main abstraction. A protocol becomes a sequence of semantic events. [Source: how-to-sans-io, "Events"](https://sans-io.readthedocs.io/how-to-sans-io.html)

## 3. How hosts drive the core, sync and async

- **One loop shape, any runtime.** str0m's README gives a six-step shape <q>regardless of sync or async</q>:
  1. Wait for the next timeout, an incoming packet, or an application mutation.
  2. Perform that one mutation.
  3. Drain `poll_output`.
  4. Handle each output (transmit, event, or timeout).
  5. Stop draining at `Output::Timeout`.
  6. Wait on the returned timeout.
  - Its example is a blocking UDP socket with a read timeout. It says <q>with async you would `select!` over multiple futures</q>, and it includes a channel for application-side data.
  - [Source: str0m README, "Canonical shape"](https://github.com/algesten/str0m/blob/main/README.md)
- **Runtime compatibility.** str0m's FAQ: <q>fully sync, ensuring that it integrates seamlessly with any Rust async runtime</q>. [Source: README FAQ](https://github.com/algesten/str0m/blob/main/README.md)
- **Time is the host's job.** The core exposes the next deadline (`poll_timeout` / `Output::Timeout`). The host sleeps until then and calls back with `now`. [Source: quinn-proto](https://github.com/quinn-rs/quinn/blob/main/quinn-proto/src/connection/mod.rs), [str0m](https://github.com/algesten/str0m/blob/main/README.md)
- **Two integration strategies**, from the principle docs:
  - Build a native-feeling wrapper for one I/O model, like aiohttp.
  - Or push I/O to the edges so one codebase drops into multiple models. This takes <q>substantial care and discipline</q> and may not <q>feel very native</q>.
  - [Source: how-to-sans-io, "Integrating With I/O"](https://sans-io.readthedocs.io/how-to-sans-io.html)
- **Layering.** quinn-proto's own docs say most users may want the futures-based `quinn` API, which wraps the sans-IO core. [Source: lib.rs](https://github.com/quinn-rs/quinn/blob/main/quinn-proto/src/lib.rs)
  - [Unknown] I did not read how the `quinn` crate's runtime abstraction is built. Its `runtime.rs` was not found at the path I tried.
- **Ownership fit.** str0m says it rejected the standard `RTCPeerConnection` callback API because callbacks need shared ownership (`Arc<Mutex<..>>`). `mpsc` channels are <q>awkward without async</q>. Pull-style events avoid both. [Source: README, "not a standard RTCPeerConnection"](https://github.com/algesten/str0m/blob/main/README.md)

## 4. Known pitfalls and contracts

1. **Drain discipline (str0m).** <q>Every mutation of an `Rtc` instance must be followed by a complete drain of `poll_output` until it returns `Output::Timeout`, before the next mutation.</q> Violating it <q>leaves the engine in an inconsistent state</q>. Mutations issued inside the drain loop are fine. [Source: README, "single-mutation invariant"](https://github.com/algesten/str0m/blob/main/README.md)
2. **Buffer and discard contract (rustls).** After handling a state, `discard` bytes must be removed from the front of the incoming buffer. This MUST happen before the next `process_tls_records` call. The returned state MUST be handled first. `TransmitTlsData` contents MUST NOT be sent twice. [Source: unbuffered.rs docs](https://github.com/rustls/rustls/blob/v/0.23.31/rustls/src/conn/unbuffered.rs)
   - The status type borrows the connection and input buffer (`UnbufferedStatus<'c, 'i, _>`). The compiler therefore enforces some of this contract. [Source: same]
3. **Deadlines change after any call (quinn-proto).** `poll_timeout` may change after I/O, `handle_event`, `poll_transmit` returning `Some`, and `handle_timeout`. Re-read it each loop iteration instead of caching it. [Source: connection/mod.rs](https://github.com/quinn-rs/quinn/blob/main/quinn-proto/src/connection/mod.rs)
4. **Host-side bugs can surface as protocol failures.** In quinn-proto, mis-using the endpoint API <q>could cause spurious connection loss</q>. The source comment is about a connection id that was drained illegally and then reallocated. [Source: endpoint.rs](https://github.com/quinn-rs/quinn/blob/main/quinn-proto/src/endpoint.rs)
5. **Clock must be monotonic and caller-supplied.** The cores take `Instant` as an argument. [Unknown] Behavior if the host passes a non-monotonic `now` is not documented in what I read.
6. **Wall-clock and media time are separate inputs (str0m).** The README describes "now" (an `Instant`), media time and wallclock as three distinct concepts. [Source: README, "Media time, wallclock and local time"](https://github.com/algesten/str0m/blob/main/README.md)
7. **Panic policy.** str0m panics on broken internal invariants (<q>panic means bug</q>) and recommends `catch_unwind` to discard a faulty `Rtc`. Because it has no threads, locks or async tasks, discarding it does not risk poisoning locks. [Source: README, "Panics, Errors and unwraps"](https://github.com/algesten/str0m/blob/main/README.md)
8. **Costs the principle docs admit.**
   - The sans-IO crate alone is <q>not very useful</q>, because the host must write the glue.
   - The result may not feel native in any I/O model.
   - [Source: how-to-sans-io](https://sans-io.readthedocs.io/how-to-sans-io.html)

## 5. Implications for an agent harness [Inference, not sourced]

The principle docs and crates are about network protocols. Applying them to an LLM agent harness is my extrapolation.

- **Natural mapping.**
  - The core would take `Input` values: user message, model chunk or response, tool result, timer tick or deadline.
  - It would emit `Output` values: "send this request to the model", "run this tool", "emit this event to UI", and "next deadline".
  - The host (sync or tokio) would do all HTTP, subprocess and filesystem work.
  - This mirrors str0m's `Input`/`Output` split and rustls's "what to do next" states.
- **Likely wins:**
  - Deterministic replay tests with scripted model responses and a fake clock.
  - No async runtime lock-in.
  - Easy pause, resume and serialize points, because state is plain data.
  - Easy fault injection.
- **Likely costs:**
  - Every host must implement the driving loop.
  - Concurrency, such as parallel tool calls and cancellation, must be modeled as explicit inputs and outputs, not as `await`.
  - Streaming token output needs an event style (pull queue or returned events).
  - A less ergonomic API than an `async fn run()`. The principle docs concede this.
- **Contracts to decide up front:**
  - A str0m-style drain rule.
  - Who owns buffers (rustls-style) or whether the core owns them.
  - How time is passed (`Instant` argument).
  - Whether a convenience async wrapper ships alongside the core, as quinn does with quinn-proto.
- **Open question:** whether LLM and tool I/O is "bytes in, bytes out" enough for the benefits to carry over. The sources do not address non-network domains.

## 6. Sources not examined

- No source on `h2`, `hyper` or `h11` Rust equivalents, because these are not needed for the question.
- [Unknown] No primary source found for sans-IO in a Rust agent or LLM-client context.
- [Unknown] I did not fetch docs.rs rendered pages. WebFetch was blocked, so I read the upstream source and doc comments, which docs.rs renders.
