# Research: Pi SDK and agent-loop architecture

Ticket: mgschiebel/libharness#2. Purpose: give later tickets a feature-match target.

## Sources and snapshot

All claims below come from Pi's own docs and source. Snapshot read: repo head `2c8d1feb147f6b3d97e855da803bfe7b19f3c403` (2026-11-01), cloned via `github.com/badlogic/pi-mono`; the docs name the canonical repo as `earendil-works/pi`. Links use `main`, so line content may drift.

- Site: https://pi.dev and https://pi.dev/docs/latest
- Repo: https://github.com/earendil-works/pi
- Agent core README: https://github.com/earendil-works/pi/blob/main/packages/agent/README.md
- Agent core types: https://github.com/earendil-works/pi/blob/main/packages/agent/src/types.ts
- Agent loop source: https://github.com/earendil-works/pi/blob/main/packages/agent/src/agent-loop.ts
- Coding-agent docs dir: https://github.com/earendil-works/pi/tree/main/packages/coding-agent/docs
  - SDK: .../docs/sdk.md ; How Pi works: .../docs/how-pi-works.md ; Extensions: .../docs/extensions.md ; Compaction: .../docs/compaction.md ; Session format: .../docs/session-format.md ; Sessions: .../docs/sessions.md ; Message types: .../docs/message-types.md ; Settings: .../docs/settings.md
- Session events source: https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/agent-session.ts

## 1. Package layering

Pi is a TypeScript monorepo. Relevant layers (https://github.com/earendil-works/pi/tree/main/packages):

| Package | Role |
|---|---|
| `pi-ai` | Provider/model streaming (`streamSimple`, message types) |
| `pi-agent-core` (`packages/agent`) | `Agent` class + low-level `agentLoop()`; stateful, tool execution, event streaming ([README](https://github.com/earendil-works/pi/blob/main/packages/agent/README.md)) |
| `pi-coding-agent` | `AgentSession`, `SessionManager`, extensions, compaction, built-in tools, CLI ([sdk.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/sdk.md)) |

So there are two SDK surfaces: the minimal `Agent`/`agentLoop` (core) and `createAgentSession()` (coding-agent, full product behavior). pi.dev describes Pi as a minimal harness running in interactive, print/JSON, RPC and SDK modes (https://pi.dev). "All interfaces use the same agent and session mechanisms" ([how-pi-works.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/how-pi-works.md#interfaces)).

## 2. Agent-loop lifecycle

Conceptual (docs): a submitted message is added to the active branch, a request is built from system prompt, branch, tools and model settings, the provider streams an assistant response (text and tool calls), Pi executes each tool call and records results; that is one turn. Another turn starts if tool results or queued messages require it, else the run ends ([how-pi-works.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/how-pi-works.md#agent-loop)).

Actual control flow in `runLoop` ([agent-loop.ts](https://github.com/earendil-works/pi/blob/main/packages/agent/src/agent-loop.ts)):

1. Poll steering queue once at start.
2. Outer loop (follow-ups) wraps inner loop (tool calls + steering).
3. Per turn: `prepareNextTurn` (not on first turn) -> poll steering if none pending -> emit `turn_start` -> emit pending messages as `message_start`/`message_end` -> `prepareRequest` -> stream assistant response.
4. If `stopReason` is `error` or `aborted`: run `finishTurn`, emit `turn_end`, `agent_end`, return (hard exit).
5. Otherwise execute tool calls (if `stopReason` is `length`, all tool calls are failed rather than executed because arguments may be truncated), append tool results, run `finishTurn`, emit `turn_end`. `{action:"end"}` emits `agent_end` and returns.
6. Poll steering. Loop continues while tool calls remain (unless every result sets `terminate`) or steering messages exist.
7. When the inner loop would stop, poll follow-ups; if any, inject and continue; else if `finishTurn` returned `continue`, make one context-only request; else emit `agent_end`.

Hook contracts and types: [types.ts](https://github.com/earendil-works/pi/blob/main/packages/agent/src/types.ts) (`AgentLoopConfig`, `FinishTurn`, `PrepareRequest`, `prepareNextTurn`). README per-turn lifecycle: input events -> `prepareRequest` -> provider response -> tool results -> `finishTurn` -> `turn_end` -> scheduling or `agent_end` ([README](https://github.com/earendil-works/pi/blob/main/packages/agent/README.md#request-preparation-and-turn-finalization)).

Two-level end signals in the coding-agent layer: `agent_end` ends one low-level run, but retry, overflow recovery, compaction retry or queued messages may start another; `agent_settled` means Pi will not continue automatically; `agent_before_settle` is the last actionable boundary and can request one continuation ([sdk.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/sdk.md#subscribing-to-events), [extensions.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md#agent_start--agent_end--agent_settled)).

Message flow: `AgentMessage[] -> transformContext() (optional) -> convertToLlm() (required) -> Message[] -> LLM`. `AgentMessage` is LLM messages plus app-defined types via TypeScript declaration merging ([README](https://github.com/earendil-works/pi/blob/main/packages/agent/README.md#core-concepts)).

The stream function contract: must not throw for request/model failures; failures are encoded in the returned stream as a final assistant message with `stopReason` `error`/`aborted` ([types.ts](https://github.com/earendil-works/pi/blob/main/packages/agent/src/types.ts), `StreamFn`).

## 3. Event types

Core `AgentEvent` ([types.ts](https://github.com/earendil-works/pi/blob/main/packages/agent/src/types.ts)): `agent_start`, `agent_end{messages}`, `turn_start`, `turn_end{message,toolResults}`, `message_start`, `message_update{assistantMessageEvent}` (assistant only), `message_end`, `tool_execution_start{toolCallId,toolName,args}`, `tool_execution_update{partialResult}`, `tool_execution_end{result,isError,durationMs?}`.

Typical sequence for a prompt with a tool call is in the [README event-flow section](https://github.com/earendil-works/pi/blob/main/packages/agent/README.md#with-tool-calls). `Agent.subscribe()` listeners are awaited in registration order and `agent_end` listeners count toward settlement; `isStreaming` stays true until they finish ([README](https://github.com/earendil-works/pi/blob/main/packages/agent/README.md#event-types)). Raw `agentLoop()` streams are observational and do not wait for handlers ([README](https://github.com/earendil-works/pi/blob/main/packages/agent/README.md#low-level-api)).

`AgentSessionEvent` adds ([agent-session.ts](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/agent-session.ts)): `agent_end.willRetry`, `agent_settled{aborted}`, `queue_update{steering,followUp}`, `compaction_start{reason: manual|threshold|overflow}`, `compaction_end`, `entry_appended`, `session_info_changed`, `thinking_level_changed`, `auto_retry_start/end`, `summarization_retry_scheduled` (list may be longer; I read only the visible head of the union).

## 4. Tool interface

`AgentTool` ([types.ts](https://github.com/earendil-works/pi/blob/main/packages/agent/src/types.ts)): `name`, `description`, `parameters` (TypeBox schema), `label` (UI), `execute(toolCallId, params, signal?, onUpdate?) -> AgentToolResult`, optional `prepareArguments` (pre-validation shim), `outputSchema`, `executionMode` ("sequential"|"parallel"), `replay` ("never"|"safe", recovery policy for effects with unknown outcome).

`AgentToolResult`: `content` (text/image to model), `details` (for UI/logs), `structuredContent`, `usage`, `isError`, `terminate`. Failure: throw, or return `isError: true`; thrown errors are reported to the model as tool errors ([README](https://github.com/earendil-works/pi/blob/main/packages/agent/README.md#error-handling)).

Execution: default `parallel` (preflight sequentially, execute allowed tools concurrently; `tool_execution_end` in completion order; tool-result messages in assistant source order). Any `sequential` tool in a batch makes the whole batch sequential ([README](https://github.com/earendil-works/pi/blob/main/packages/agent/README.md#with-tool-calls)).

Hooks: `beforeToolCall` runs after `tool_execution_start` and argument validation, can `block` (with `reason`, `terminate`); `afterToolCall` runs before `tool_execution_end`, can override `content`, `details`, `isError`, `usage`, `terminate`, `structuredContent` field-by-field with no deep merge ([types.ts](https://github.com/earendil-works/pi/blob/main/packages/agent/src/types.ts)). `terminate: true` stops the loop only if every finalized result in the batch has it ([README](https://github.com/earendil-works/pi/blob/main/packages/agent/README.md#with-tool-calls)).

Built-in tools in the coding agent: `read`, `bash`, `powershell`, `edit`, `write`, `grep`, `find`, `ls`; defaults `read, bash, edit, write` ([settings.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/settings.md)). MCP and codemode are optional extensions, not core; SDK sessions must add them explicitly ([sdk.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/sdk.md#codemode-mcp)).

## 5. Steering and follow-up queueing

- `agent.steer(msg)`: delivered after the current assistant turn's tool calls have all finished; the LLM responds on the next turn; tool calls are not skipped ([README](https://github.com/earendil-works/pi/blob/main/packages/agent/README.md#steering-and-follow-up), `getSteeringMessages` doc in [types.ts](https://github.com/earendil-works/pi/blob/main/packages/agent/src/types.ts)).
- `agent.followUp(msg)`: polled only when there are no more tool calls and no steering; if present, injected and another turn runs.
- `steeringMode` / `followUpMode`: `"one-at-a-time"` (default) or `"all"` (`QueueMode` in types.ts; defaults also in [settings.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/settings.md)).
- Clear APIs: `clearSteeringQueue`, `clearFollowUpQueue`, `clearAllQueues`; `peekQueuedMessages()`.
- `continue()`: resumes from a non-assistant tail; if tail is assistant, falls back to one queued steering batch then one follow-up batch ([README](https://github.com/earendil-works/pi/blob/main/packages/agent/README.md#continue-and-queued-input)).
- Session layer: `prompt()` while streaming must specify steer or follow-up, else it rejects; `steer()`/`followUp()` return `"queued"` or `"handled"` (an extension consumed it) ([sdk.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/sdk.md#prompting)). Aborting returns queued messages to the editor ([how-pi-works.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/how-pi-works.md#agent-loop)).

## 6. Session model

- Sessions are append-only JSONL, one tree per file; entries carry `id`/`parentId`; header line holds `version` (current 3), `cwd`, optional `parentSession`. Default path `~/.pi/agent/sessions/--<path>--/<timestamp>_<id>.jsonl` ([session-format.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/session-format.md)).
- Entry types: `session` header, `message`, `model_change`, `thinking_level_change`, `usage`, `compaction`, `context_edit`, `branch_summary`, `custom` (extension state, not in LLM context), `custom_message` (in LLM context), `label`, `session_info` (same doc).
- System prompt and tool declarations are themselves persisted as `system` messages: a first full one, then patches by section name and `toolsAdded`/`toolsRemoved`; replay yields current state ([session-format.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/session-format.md#sessionmessageentry)).
- Branching: continuing from an earlier entry makes another branch in the same file; `/fork` and `/clone` copy history into a new file; the active branch (leaf to root) supplies model context ([how-pi-works.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/how-pi-works.md#sessions), [sessions.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/sessions.md)).
- `context_edit` is an append-only, branch-relative edit that omits or replaces one earlier entry's contribution to future context without altering raw history.
- SDK: `SessionManager` is authoritative for finalized context; in-memory variant `SessionManager.inMemory()`; assigning `session.agent.state.messages` does not replace persisted context. `AgentSessionRuntime` adds `newSession`, `switchSession`, `fork`, `importFromJsonl`, each replacing the active `AgentSession` ([sdk.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/sdk.md#session-storage)).
- `createAgentSession()` options: `modelRuntime`, `model`, `thinkingLevel`, `scopedModels`, `settingsManager`, `sessionManager`, `resourceLoader`, `tools`/`noTools`/`excludeTools`/`customTools`, `cwd` ([sdk.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/sdk.md#configuring-a-session)).

## 7. Compaction

All from [compaction.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/compaction.md):

- Trigger: `contextTokens > contextWindow - reserveTokens` (default `reserveTokens` 16384); manual `/compact [instructions]`. Checked before a new prompt, between turns after tool results (in `prepareNextTurn`), and on provider context-overflow or early `stopReason: "length"` (one compact-and-retry).
- Method: walk back accumulating token estimates until `keepRecentTokens` (default 20k); summarize everything before the cut with an LLM (previous summary passed as iterative context); append a `CompactionEntry{summary, firstKeptEntryId, tokensBefore, usage, details}`; context is rebuilt as summary + messages from `firstKeptEntryId`. Original entries are never deleted.
- Cut rules: valid cut points are user, assistant, bash-execution and custom messages; never at tool results. If a single user-message span exceeds budget, it splits and two summaries are merged.
- Summary format: Goal, Constraints & Preferences, Progress, Key Decisions, Next Steps, Critical Context; file read/modified lists tracked cumulatively in `details`.
- Branch summarization: on `/tree` navigation Pi can summarize the abandoned branch up to the common ancestor and append a `BranchSummaryEntry`.
- Extension hooks: `session_before_compact` (cancel or customize), `session_compact`, `session_compact_failed`, `session_before_tree`, `session_tree` ([extensions.md lifecycle table](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md#understand-the-lifecycle)). Also `ctx.compact()` and `ctx.getContextUsage()` in the ExtensionContext. Compaction and overflow-recovery ordering is specified in the "Overflow and Length Recovery Ordering" section of compaction.md.

## 8. Extension hook points

Extensions are TypeScript modules loaded in-process; a factory registers tools, commands, shortcuts, providers, event handlers, renderers, UI ([how-pi-works.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/how-pi-works.md#extensions-and-resources)). Event catalogue ([extensions.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md#events)):

- Startup/resources: `project_trust`, `resources_discover`, `mcp_servers_change`
- Session: `session_start`, `session_info_changed`, `session_before_switch`, `session_before_fork`, `session_before_compact`/`session_compact`/`session_compact_failed`, `session_before_tree`/`session_tree`, `session_shutdown`
- Agent: `input` (intercept/transform/handle), `before_agent_start` (inject message, patch system prompt sections), `agent_start`, `agent_end`, `agent_before_settle`, `agent_settled`, `turn_start`, `turn_end` (can propose entries and continuation), `message_start/update/end`, `tool_execution_*`, `context`, `context_with_system`, `before_provider_headers`, `before_provider_request`, `after_provider_response`, `provider_stream_event`, `cache_warming_decision`, `ui_prompt_start/end`
- Model: `model_select`, `thinking_level_select`
- Tool: `tool_call` (block or mutate input), `tool_result` (modify result)
- User bash: `user_bash`

Composition rules: handlers run in load then registration order; patterns are notification, in-place mutation, chained transformation, gate, actionable boundary, last-override ([extensions.md](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md#handler-composition)). Draft entry types allowed from `turn_end`/`agent_before_settle`: `custom`, `custom_message`, `context_edit`, `compaction`; validated before append but "persistence is not transactional".

Extension API surface (headings in extensions.md): `pi.on`, `registerTool`, `sendMessage`, `sendUserMessage`, `appendEntry`, `setSessionName`, `setLabel`, `registerCommand`, `registerShortcut`, `registerFlag`, `exec`, `getActiveTools/setActiveTools`, `setModel`, `setThinkingLevel`, `events` (inter-extension bus), `registerProvider`, `registerVirtualModel`, `registerMcpServer`, `registerToolRenderer`, `getSettings`. Command context adds `newSession`, `fork`, `navigateTree`, `switchSession`, `reload`, `waitForIdle`.

Core-level (non-extension) equivalents for a library: `transformContext`, `convertToLlm`, `prepareRequest`, `prepareNextTurn`, `finishTurn`, `beforeToolCall`, `afterToolCall`, `getSteeringMessages`, `getFollowUpMessages`, `getApiKey` ([types.ts](https://github.com/earendil-works/pi/blob/main/packages/agent/src/types.ts)).

## 9. Takeaways for feature-matching (derived, not from Pi)

- The cleanest portable core is `agent-loop.ts`: two nested loops, six hooks, queue polling at fixed points. Steering/follow-up are polled via callbacks, not channels.
- Pi keeps persisted session (tree, JSONL) separate from the in-memory `Agent`; compaction and branching live at the session layer, not in the loop (except `prepareNextTurn`/`prepareRequest` as integration points).
- `agent_end` is not final; a distinct "settled" signal exists because retries/recovery/queues can restart runs.

## Unknowns / not verified

- Full `AgentSessionEvent` union beyond the part read; full extension event payload shapes (only a subset read).
- Whether pi.dev/docs/latest pages match the repo docs one-to-one (the pi.dev page returned HTTP 200 but its contents were not diffed; repo docs were treated as authoritative).
- The `Agent` class implementation ([agent.ts](https://github.com/earendil-works/pi/blob/main/packages/agent/src/agent.ts), 623 lines) was not read in detail; queue and barrier behavior is taken from the README and `agent-loop.ts`.
- GitHub links were not fetched live (proxy returned 403 to curl); content was read from a local clone at the SHA above.
