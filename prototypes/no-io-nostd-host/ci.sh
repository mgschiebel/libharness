#!/usr/bin/env bash
# PROTOTYPE CI-style gate for the no-I/O core. Run from this directory.
set -u
cd "$(dirname "$0")"
T=wasm32v1-none
fail=0
step() { echo "--- $1"; shift; "$@" >/tmp/nostd-ci.log 2>&1 && echo "PASS" || { echo "FAIL"; tail -${CI_TAIL:-6} /tmp/nostd-ci.log | sed 's/^/    /'; fail=1; }; }
step "1. cargo check core (std-free target, schemars)"  cargo check -p nostd-core --target $T
step "1b. cargo check core (hand-rolled schema, no default features)" cargo check -p nostd-core --target $T --no-default-features
step "2. cargo deny check bans (allowlist + build scripts)" cargo deny --exclude host-tests --exclude host check bans
step "2b. no build script in workspace members" python3 -I nobuild.py
step "3. probe cdylib builds + zero wasm imports" bash -c "cargo build -p probe --release --target $T && python3 -I imports.py target/$T/release/probe.wasm"
exit $fail
