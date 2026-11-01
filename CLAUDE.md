# libharness

An LLM harness as a Rust library.

## Agent skills

### Issue tracker

Issues are tracked in GitHub Issues for `mgschiebel/libharness` (via `gh`). See `docs/agents/issue-tracker.md`.

### Triage labels

Default vocabulary: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `GLOSSARY.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.

## Interaction rules

- Ask questions inline in chat as plain text. Never use menu or multiple-choice question tools (e.g. AskUserQuestion). The user works by voice-to-text, which menus break.
