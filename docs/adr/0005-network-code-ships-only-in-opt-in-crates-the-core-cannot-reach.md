---
status: accepted
---

# Network code ships only in opt-in crates the core cannot reach; the OpenRouter client is our own

The library holds no network code of its own. The OpenRouter wire format and the code that runs one model call live in the gated core and talk to the outside only through a Transport capability the host registers. Anything that touches the network ships as an opt-in crate outside the gate, and the core can never reference it. We write our own OpenRouter client instead of reusing an existing one. Decided 2026-11-14 in the build-vs-adopt grilling, starting from the recommendation in issue 34, on top of ADRs 0001 to 0004. The model is Lua's: the host opens the standard libraries, and nothing is registered by default.

## What this settles

- **Codec.** The OpenRouter codec is a module in `libharness-codec` (request becomes an HTTP request as data, bytes become events). It replaces the std adapter crate `libharness-openrouter` from ADR 0002.
- **Exchange.** The code that runs one model call (send the codec's request through the Transport, feed the reply bytes to the decoder, map failures to the structured error) is pure async code in a new gated crate, `libharness-exchange`. A scratch port of the prototype's version built under `wasm32v1-none` on 2026-11-14 using only `core` and `alloc`; it is checked again when the real crate is built.
- **Transport.** A host capability: an HTTP request as data in; status, headers and a byte stream out. A host with its own HTTP client gets the whole OpenRouter model call by registering only its Transport. The exact callback shape, where the API key lives and the retry sleeper belong to the host-interface grilling (issue 8).
- **Opt-in ureq transport.** A separate ungated crate, `libharness-transport-ureq` (`gated = false`), with the thread bridge and thread sleeper inside it. It depends on the core and never the reverse, and the host registers it explicitly. It is a crate and not a cargo feature because ADR 0002 forbids features on gated crates (features can unify and switch `std` on in the gated tree). The cargo-deny allowlist on the gated crates should fail the build if one ever depends on it; to be confirmed when the check is written.
- **Licences.** The ureq crate has its own cargo-deny allowlist: the permissive set from ADR 0001 plus ISC, BSD-3, Zlib, Unicode-3.0 and CDLA-Permissive-2.0. It stays permissive-only.
- **No reqwest crate in the first release.** A reqwest transport example (about 40 lines in the prototype) goes in `examples/`, built in CI and never published.
- **Runtime-neutral.** Nothing the library ships starts a private async runtime.
- **Own client.** The library depends on none of genai, openrouter-rs, async-openai or openrouter_api.
- **Release process.** A spec diff of the chat-completions streaming slice of OpenRouter's OpenAPI file before each release. A live OpenRouter smoke test, recorded next to the Anthropic one, is required before the first public release: until then TLS is untested and every result comes from loopback mocks.

## Considered options

- **Reuse genai or openrouter-rs.** Rejected: both need a tokio runtime and panic without one (genai at TCP connect, openrouter-rs at first poll), they bring 114 to 145 packages with 16 to 17 build scripts, and each hides or loses part of OpenRouter's responses (cost, served model, streamed reasoning details, mid-stream errors). Only a private runtime inside an adapter makes them executor-neutral, which was ruled out as the default.
- **A std adapter crate holding the transport** (the layout in ADR 0002). Rejected: the library should not hold network access of its own. The transport is a callback the host registers, and the piece we ship is opt-in.
- **A reqwest transport crate.** Not in the first release: it works only inside tokio, and a host on tokio can register its own in about 40 lines.

## Consequences

- We own the OpenRouter codec and its drift from the spec, instead of a crate's churn.
- The ureq transport costs one OS thread and one new connection per call (a dropped call cannot otherwise close its socket), and about 26 packages with 7 build scripts, all outside the gate.
- The glossary gains **Transport** and **Exchange** and drops **Adapter**.
- Carried to the host-interface grilling (issue 8): the Transport callback shape (the body is lent to the future), where the API key lives (lean: the host's transport adds it), and the retry sleeper. Carried to the provider-abstraction grilling (issue 9): reserved `extra` keys, the structured error fields, and the `reasoning_details` merge, which touches the replay-verbatim rule in ADR 0004.
