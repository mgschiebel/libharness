#!/usr/bin/env bash
# The no-I/O gate (ADR 0002). Runs every layer on every gated crate and reports each result.
# Exit status is non-zero if any layer failed. Run from anywhere; it changes to the repo root.
#
# Layers, per gated crate unless noted:
#   1. crate classification and source rules   (ci/crates.py: gated declared, no build script, no_std, forbid unsafe)
#   2. per-package wasm32v1-none check          (never workspace-wide: feature unification can switch std on)
#   3. cargo-deny allowlist and licences        (workspace-wide, ungated crates excluded)
#   4. clippy await_holding_invalid_type        (a spin guard must not be held across an .await)
#   5. zero-import wasm probe                   (once)
#
# CI_TAIL=<n> sets how many log lines a failing layer prints (default 8).
set -u
cd "$(dirname "$0")/.."

TARGET=wasm32v1-none
LOG=$(mktemp)
trap 'rm -f "$LOG"' EXIT
fail=0

step() {
    local label=$1
    shift
    if "$@" >"$LOG" 2>&1; then
        echo "PASS  $label"
    else
        echo "FAIL  $label"
        tail -n "${CI_TAIL:-8}" "$LOG" | sed 's/^/        /'
        fail=1
    fi
}

step "1. crate classification and source rules" python3 -I ci/crates.py check

# If classification failed the crate list cannot be trusted, so stop here.
if ! gated=$(python3 -I ci/crates.py gated 2>/dev/null) || [ -z "$gated" ]; then
    echo "      no usable list of gated crates; later layers skipped"
    exit 1
fi
ungated=$(python3 -I ci/crates.py ungated)

for crate in $gated; do
    step "2. wasm32v1-none check: $crate" cargo check -p "$crate" --target "$TARGET"
done

exclude=()
for crate in $ungated; do exclude+=(--exclude "$crate"); done
step "3. cargo-deny bans and licences" cargo deny "${exclude[@]}" check bans licenses

# Clippy does not re-run when only clippy.toml changes, so make every gated crate look modified.
for crate in $gated; do
    root=$(cargo metadata --no-deps --format-version 1 | python3 -I -c \
        'import json,sys; n=sys.argv[1]; print(next(t["src_path"] for p in json.load(sys.stdin)["packages"] if p["name"]==n for t in p["targets"] if "lib" in t["kind"]))' "$crate")
    touch "$root"
    step "4. clippy await_holding_invalid_type: $crate" cargo clippy -p "$crate" --target "$TARGET" -- -D warnings -D clippy::await_holding_invalid_type
done

step "5. zero-import wasm probe" bash -c \
    "cargo build -p libharness-probe --release --target $TARGET && python3 -I ci/wasm_imports.py target/$TARGET/release/libharness_probe.wasm"

if [ "$fail" -eq 0 ]; then echo "gate: all layers passed"; else echo "gate: FAILED"; fi
exit "$fail"
