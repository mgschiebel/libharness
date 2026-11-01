#!/usr/bin/env bash
# PROTOTYPE: the full enforcement pipeline (all three layers). Fails on first failing layer.
set -euo pipefail
cargo check --workspace --quiet                  # layer 1: forbid(unsafe_code) (rustc)
cargo clippy --workspace --quiet -- -D warnings  # layer 2: generated disallowed_* denylist
cargo deny check bans sources                    # layer 3: allowlist, build scripts, sources
