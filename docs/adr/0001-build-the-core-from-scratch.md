---
status: accepted
---

# Build the core from scratch

The core is written from scratch rather than built on an existing agent crate (rig, swiftide and similar) or contributed upstream. Those crates ship bundled with their own provider layer and perform I/O themselves, so they fail the no-I/O dependency allowlist; rig also lacks mid-run steering and owns its side effects, which does not fit the session-as-task model. Decided 2026-11-01 in the build-vs-adopt grilling, starting from the recommendation in issue 15.

## Rules that follow

- **Dependencies.** "From scratch" means no agent framework and no provider client in the core. Small utility crates are allowed if they pass the no-I/O allowlist gate.
- **Licences.** Core dependencies are limited to permissive licences (MIT, Apache-2.0, BSD and similar), enforced in CI by cargo-deny next to the no-I/O allowlist. The project's own licence is a separate question.
- **MCP.** MCP needs I/O transports, so it stays on the host side: the host uses any MCP client it likes and registers each MCP tool as an ordinary capability. The library has no MCP build-or-adopt question.
- **Concepts only.** Other projects are references for ideas; no code is copied, because of licence pollution and unverified licence compatibility. No upstream contribution is part of this effort.

## References

Ideas only, no code:

- **Pi**: the two-layer shape, nested loops with hooks, and steering and follow-up queues.
- **rig**: compared against the libharness requirements (issue 10); informed the rejection of an ergonomic agent type that owns side effects.
- **genai, async-openai**: provider-client designs and message shapes; candidates for the optional adapter, not dependencies of the core.
- **swiftide**: compared in the crate-landscape research (issue 5).

## Left open

Whether the optional OpenRouter adapter reuses an existing client crate or is written from scratch, and where the adapter lives, are decided by a research and prototype round before this grilling closes.
