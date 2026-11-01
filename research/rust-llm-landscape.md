# Rust LLM crate landscape

Ticket: [#5 Research: Rust LLM crate landscape](https://github.com/mgschiebel/libharness/issues/5)
Researched: 2026-11-01

**Question.** What existing Rust crates cover LLM provider clients and agent loops, and where are the gaps libharness fills? Scope, maintenance status, provider abstraction approach and licence for each.

## Method and limits

- Sources used: the crates.io API for each crate (latest version, publish date, SPDX licence field, repository link), and each crate's own repository on `main`/`master` (README, LICENSE file, CHANGELOG where present).
- **Not verified:** docs.rs pages were not read (links are given but no claim below rests on them). GitHub repo metadata (last commit date, archived flag, star count) was unreachable from this session, so "maintenance" below means *latest crates.io release date plus changelog activity*, not commit history. Marked `unknown` where that matters.
- "Today" is 2026-11-01. Crate versions and dates are as of that day and will drift.
- The libharness repo defines its scope only as "An LLM harness as a Rust library" ([CLAUDE.md](https://github.com/mgschiebel/libharness/blob/main/CLAUDE.md)). There is no GLOSSARY or ADR yet, so the "gaps" section is provisional.

## Summary table

| Crate | Latest (crates.io) | Released | Licence | Scope | Provider abstraction |
|---|---|---|---|---|---|
| [rig-core](https://crates.io/crates/rig-core) / [rig](https://crates.io/crates/rig) | 0.46.0 | 2026-10-31 | MIT | Provider layer + agent runtime + memory + tools + coding agent | Provider-neutral traits in `rig-core` |
| [genai](https://crates.io/crates/genai) | 0.7.2 | 2026-10-28 | MIT OR Apache-2.0 | Provider client only (chat, stream, embed) | Adapters chosen by model name or `ns::model` |
| [async-openai](https://crates.io/crates/async-openai) | 0.43.0 | 2026-11-01 | MIT | OpenAI API client (very wide surface) | Single provider; BYO types for OpenAI-compatible endpoints |
| [swiftide](https://crates.io/crates/swiftide) | 0.33.0 | 2026-10-14 | MIT | Agent harness + task graphs + RAG pipelines | Per-provider integrations behind features |
| [adk-rust](https://crates.io/crates/adk-rust) | 2.3.0 (3.0 RC unpublished) | 2026-10-09 | Apache-2.0 | Agent framework, 40+ crates, server, sessions | Swap a client struct; agent unchanged |
| [langchain-rust](https://crates.io/crates/langchain-rust) | 7.0.0 | 2026-10-22 | MIT | LangChain port: chains, agents, vector stores | Per-provider LLM types |
| [llm](https://crates.io/crates/llm) (graniet) | 1.3.8 | 2026-04-19 | MIT | Multi-backend client, chains, agents, REST server | `ChatProvider` / `CompletionProvider` traits |
| [ollama-rs](https://crates.io/crates/ollama-rs) | 0.3.6 | 2026-07-24 | see below | Ollama API client | Single provider |
| [llm-chain](https://crates.io/crates/llm-chain) | 0.13.0 | 2023-11-15 | MIT | Chains of LLM calls | n/a (stale) |

Crates.io data: [rig-core](https://crates.io/api/v1/crates/rig-core), [genai](https://crates.io/api/v1/crates/genai), [async-openai](https://crates.io/api/v1/crates/async-openai), [swiftide](https://crates.io/api/v1/crates/swiftide), [adk-rust](https://crates.io/api/v1/crates/adk-rust), [langchain-rust](https://crates.io/api/v1/crates/langchain-rust), [llm](https://crates.io/api/v1/crates/llm), [ollama-rs](https://crates.io/api/v1/crates/ollama-rs), [llm-chain](https://crates.io/api/v1/crates/llm-chain).

## Per-crate findings

### rig (rig-core, rig-agent, ...)

- **Scope.** The README describes it as "a Rust library for building LLM applications, and `rig`, a terminal coding agent built on it that you extend with Bevy plugins" ([README](https://github.com/0xPlaygrounds/rig/blob/main/README.md)). It is a workspace of crates: `rig-core` (provider-neutral messages, completion and embedding models, tool contracts, vector-store contracts, providers), `rig-agent` (agent builder, streaming, hooks, tool registry, serialisable `AgentRun`), `rig-memory` (windows, compaction, save/resume/fork), `rig-cassette` (effect record/replay/checkpoints), `rig-tools` (read/write/edit/search/shell), `rig-ecs`/`rig-harness` (agent kernel on Bevy ECS) ([README, Crates table](https://github.com/0xPlaygrounds/rig/blob/main/README.md)). It claims "20+ model providers and 10+ vector stores, each under one interface" and OpenTelemetry GenAI semantic conventions (same README).
- **Surprise (relevant to libharness).** Rig has moved from "LLM app library" toward a *harness* of its own: sessions, compaction, checkpoints, plugins, MCP client and ACP server ([CHANGELOG 0.46.0](https://github.com/0xPlaygrounds/rig/blob/main/CHANGELOG.md)). It is the closest overlap with libharness found.
- **Maintenance.** 0.46.0 published 2026-10-31 ([crates.io](https://crates.io/api/v1/crates/rig-core)); changelog is generated per release PR ([CHANGELOG](https://github.com/0xPlaygrounds/rig/blob/main/CHANGELOG.md)). Active. Pre-1.0 and says so: "Releases contain breaking changes" ([README](https://github.com/0xPlaygrounds/rig/blob/main/README.md)).
- **Provider abstraction.** Provider-neutral traits in `rig-core`, providers under `rig::providers::*`, agent built from a model value (`AgentBuilder::new(model)`) ([README](https://github.com/0xPlaygrounds/rig/blob/main/README.md)). Docs: [docs.rs/rig-core](https://docs.rs/rig-core) (not read).
- **Licence.** MIT ([LICENSE](https://github.com/0xPlaygrounds/rig/blob/main/LICENSE), copyright Playgrounds Analytics Inc.; crates.io `MIT`).
- **Naming caveat.** The `rig` crate on crates.io is now the facade/coding agent (0.46.0); `rig-core` is the long-standing library crate name ([crates.io rig](https://crates.io/api/v1/crates/rig)).

### genai

- **Scope.** "A Native-Protocol Multi-AI Provider Library for Rust": one API for chat, streaming and embeddings across a long list of adapters (openai, openai_resp, anthropic, gemini, ollama, vertex, bedrock, groq, deepseek, mistral, open_router and many more) ([README](https://github.com/jeremychone/rust-genai/blob/main/README.md)). No agent loop is described in the README; it is a client library.
- **Maintenance.** 0.7.2 published 2026-10-28 ([crates.io](https://crates.io/api/v1/crates/genai)); [CHANGELOG](https://github.com/jeremychone/rust-genai/blob/main/CHANGELOG.md) shows active fixes and new adapters (v0.7.0 on 2026-10-19, v0.7.2 on 2026-10-27). Pre-1.0 with breaking changes (for example `Client::new()` became fallible in 0.7.0, same changelog).
- **Provider abstraction.** Model name selects the adapter ("Common models from OpenAI, Anthropic, and Gemini can be accessed by name"), or force one with `namespace::model`; custom endpoints and auth through `ServiceTargetResolver`; "native protocols when available, OpenAI-compatible APIs when appropriate" ([README](https://github.com/jeremychone/rust-genai/blob/main/README.md)). New adapters add `AdapterKind` variants, which the changelog flags as breaking for exhaustive matches.
- **Licence.** MIT OR Apache-2.0 ([LICENSE-MIT](https://github.com/jeremychone/rust-genai/blob/main/LICENSE-MIT), [LICENSE-APACHE](https://github.com/jeremychone/rust-genai/blob/main/LICENSE-APACHE)).

### async-openai

- **Scope.** "An unofficial Rust library for OpenAI, based on OpenAI OpenAPI spec", with feature flags per API area (Responses, Chat Completions, Realtime, Audio, Images, Embeddings, Batch, Files, Fine-tuning, Administration and more); retries with backoff on rate limits, SSE streaming, WASM, tower middleware ([README](https://github.com/64bit/async-openai/blob/main/async-openai/README.md)). Client only; no agent loop.
- **Maintenance.** 0.43.0 published 2026-11-01 ([crates.io](https://crates.io/api/v1/crates/async-openai)). Changelog file: not located on `main` (`async-openai/CHANGELOG.md` returned 404); `unknown` whether one exists elsewhere.
- **Provider abstraction.** None across vendors. For OpenAI-compatible providers it offers "bring your own custom types", per-request path/query/header customisation, and Azure OpenAI ([README](https://github.com/64bit/async-openai/blob/main/async-openai/README.md)). It does not speak Anthropic or Gemini natively.
- **Licence.** MIT ([LICENSE](https://github.com/64bit/async-openai/blob/main/LICENSE), copyright Himanshu Neema).

### swiftide

- **Scope.** "Composable LLM agents and harness, typed task graphs, and streaming RAG pipelines in Rust". Agents "loop over LLM calls, tool calls, lifecycle hooks, and stop conditions"; pause/resume for human approval or persisted state; tools from Rust functions, `Tool` impls or MCP servers; tracing, metrics, Langfuse ([README](https://github.com/bosun-ai/swiftide/blob/master/README.md)). Also the other crate here that names itself a "harness".
- **Maintenance.** 0.33.0 published 2026-10-14 ([crates.io](https://crates.io/api/v1/crates/swiftide)). Pre-1.0 (version 0.x). Changelog not read; `unknown`.
- **Provider abstraction.** Provider integrations behind Cargo features (for example `--features swiftide-agents,openai`; "OpenAI-compatible integration") ([README](https://github.com/bosun-ai/swiftide/blob/master/README.md)). The extent of non-OpenAI-family coverage was not verified here.
- **Licence.** MIT ([LICENSE](https://github.com/bosun-ai/swiftide/blob/master/LICENSE), Bosun.ai).

### adk-rust

- **Scope.** "A production-ready Rust framework for building AI agents", 49 publishable crates, with runner, sessions, SQLite checkpointers, server, guardrails, eval, graph workflows ([README](https://github.com/zavora-ai/adk-rust/blob/main/README.md)). The README states a v3.0.0 release candidate is **unpublished**; the published release is 2.3.0 (same README).
- **Maintenance.** 2.3.0 published 2026-10-09 ([crates.io](https://crates.io/api/v1/crates/adk-rust)). Active; breaking 3.0 pending (README).
- **Provider abstraction.** "Swap the provider by swapping the client. The agent, runner and tools are unchanged": distinct client structs (`OpenAIClient`, `AnthropicClient`, Gemini default) behind feature flags ([README](https://github.com/zavora-ai/adk-rust/blob/main/README.md)).
- **Licence.** Apache-2.0 ([LICENSE](https://github.com/zavora-ai/adk-rust/blob/main/LICENSE); crates.io `Apache-2.0`).

### langchain-rust

- **Scope.** Rust port of LangChain: LLMs (OpenAI, Azure, Ollama, Claude), embeddings, vector stores, chains, agents (tool agent, ReAct, streaming events) ([README](https://github.com/Abraxas-365/langchain-rust/blob/main/README.md)).
- **Maintenance.** 7.0.0 published 2026-10-22 ([crates.io](https://crates.io/api/v1/crates/langchain-rust)). Active by release date; changelog not read.
- **Provider abstraction.** Per-provider LLM types listed individually in the README; trait details not verified. Docs: [docs.rs/langchain-rust](https://docs.rs/langchain-rust) (not read).
- **Licence.** MIT ([LICENSE](https://github.com/Abraxas-365/langchain-rust/blob/main/LICENSE)).

### llm (graniet)

- **Scope.** Multi-backend client (OpenAI, Anthropic, Ollama, DeepSeek, xAI, Groq, Google, Cohere, Mistral and others), plus chains, function calling, validation, retry/backoff, "Agentic" cooperating agents, and a REST server ([README](https://github.com/graniet/llm/blob/main/README.md)). The README notes the crate name previously belonged to the archived `rustformers/llm`.
- **Maintenance.** 1.3.8 published 2026-04-19 ([crates.io](https://crates.io/api/v1/crates/llm)); about six months old at time of writing. Slower than the above.
- **Provider abstraction.** Two traits, `ChatProvider` and `CompletionProvider`, with backends under feature flags (README).
- **Licence.** MIT ([LICENSE](https://github.com/graniet/llm/blob/main/LICENSE)).

### ollama-rs

- Ollama-only API client; 0.3.6 published 2026-07-24 ([crates.io](https://crates.io/api/v1/crates/ollama-rs), [README](https://github.com/pepperoni21/ollama-rs/blob/master/README.md)).
- **Licence discrepancy.** crates.io reports the SPDX field as `non-standard` with no licence file, while the repository has [LICENSE.md](https://github.com/pepperoni21/ollama-rs/blob/master/LICENSE.md) containing MIT text ("MIT License, Copyright (c) 2023 pepperoni21"). Treat as MIT-in-repo but with missing SPDX metadata; confirm before depending on it.

### llm-chain (stale)

- 0.13.0 published 2023-11-15 ([crates.io](https://crates.io/api/v1/crates/llm-chain)); repo [sobelio/llm-chain](https://github.com/sobelio/llm-chain), MIT. No release in nearly three years. Archived status `unknown` (GitHub metadata unreachable; README had no archive/deprecation notice matches).

### Other notable finds (not examined in depth)

- [`rmcp`](https://crates.io/crates/rmcp) 3.8.0, the Model Context Protocol Rust SDK in the `modelcontextprotocol` org, Apache-2.0, 2026-10-26 ([crates.io](https://crates.io/api/v1/crates/rmcp)). Relevant if libharness speaks MCP.
- [`anthropic-agent-sdk`](https://crates.io/crates/anthropic-agent-sdk) 0.2.75, MIT: wraps the Claude Code CLI over a subprocess, last release 2025-12-23 ([crates.io](https://crates.io/api/v1/crates/anthropic-agent-sdk)). Unofficial.
- [`edgequake-llm`](https://crates.io/crates/edgequake-llm) 0.10.9, Apache-2.0, multi-provider with caching, rate limiting and cost tracking, 2026-10-01 ([crates.io](https://crates.io/api/v1/crates/edgequake-llm)). Not read beyond metadata.
- Single-vendor Anthropic clients (`async-anthropic` 0.6.0, `anthropic-ai-sdk` 0.2.27, `misanthropy` 0.0.8) are unofficial; the first and last have had no release since mid-2025 ([crates.io search](https://crates.io/api/v1/crates?q=anthropic%20claude%20sdk)).
- No official Anthropic or OpenAI Rust SDK was found in the crates.io searches run (`unknown` whether one exists outside crates.io).

## Observations

1. **Provider clients are well covered.** genai (broad, native-protocol), rig-core (broad, trait-based), async-openai (deep on one vendor). A new provider client would duplicate these.
2. **Agent loops exist and are maturing, but mostly inside larger frameworks.** rig, swiftide, adk-rust and langchain-rust each bundle the loop with their own provider layer, memory/RAG story and (for rig, swiftide, adk-rust) hooks and pause/resume. All the active ones are pre-1.0 or mid-major-bump and break between releases.
3. **Coupling pattern.** Loop crates carry their own provider abstraction (rig, swiftide, adk-rust, llm); client crates (genai, async-openai) carry no loop. No crate found layers a loop cleanly over a *separately chosen* provider client.
4. **Licences are permissive throughout** (MIT, MIT OR Apache-2.0, Apache-2.0). The only flag is ollama-rs's missing SPDX metadata.

## Provisional gaps libharness could fill

Provisional, because libharness's intended scope is not yet written down.

- A **small, provider-agnostic harness core** (loop, tool dispatch, stop conditions, context/session handling) that is decoupled from any one provider client, for example by sitting on a trait that genai or rig-core could implement. Rig is moving to a full coding-agent product and swiftide/adk-rust to full frameworks (RAG, graphs, servers); a lean library is a different niche.
- **Stability**: a deliberately small public API, given every active competitor is pre-1.0 and documents breaking changes.
- Anything in the user's own design that the above lack. This cannot be assessed until requirements exist; **the main risk is rig**, which already ships hooks, memory/compaction, checkpoints, MCP and tools. A build-vs-adopt decision against rig should be an explicit follow-up question.

## Open questions for follow-up tickets

- What does libharness need that rig-agent plus rig-core does not provide? (needs requirements)
- Is genai a suitable provider backend (licence fine; API still breaking between minors)?
- docs.rs API-level comparison (trait shapes, streaming and tool-call types, error handling) was not done here.
- Maintenance by commit activity, issue responsiveness and bus factor was not measured (GitHub API unreachable).
