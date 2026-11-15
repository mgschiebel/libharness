#!/usr/bin/env bash
# Negative checks for the no-I/O gate (ADR 0002): inject one deliberate violation at a time, run
# ci/gate.sh, and require that the layer meant to catch it reports FAIL. A gate that has never been seen
# to fail proves nothing. Starts with a positive control (the clean tree must pass), restores every file it
# touches on exit, and exits non-zero if any violation slips through.
#
# Results as last recorded: docs/no-io-gate-negative-checks.md
set -u
cd "$(dirname "$0")/.."

CORE=crates/libharness
PROBE=crates/libharness-probe
SAVED=$(mktemp -d)
EVIL=$(mktemp -d)
files=("$CORE/Cargo.toml" "$CORE/src/lib.rs" "$PROBE/Cargo.toml" "$PROBE/src/lib.rs" deny.toml Cargo.lock)

for f in "${files[@]}"; do mkdir -p "$SAVED/$(dirname "$f")"; cp "$f" "$SAVED/$f"; done
restore() {
    for f in "${files[@]}"; do cp "$SAVED/$f" "$f"; done
    rm -f "$CORE/build.rs"
}
cleanup() { restore; rm -rf "$SAVED" "$EVIL"; }
trap cleanup EXIT

missed=0
total=0

# expect "<name>" "<layer prefix that must FAIL>"   (the violation is already applied)
expect() {
    local name=$1 layer=$2 out status
    total=$((total + 1))
    out=$(CI_TAIL=0 ./ci/gate.sh 2>&1)
    status=$?
    if [ "$status" -ne 0 ] && grep -q "^FAIL  $layer" <<<"$out"; then
        echo "caught   $name  -> $(grep '^FAIL' <<<"$out" | sed 's/^FAIL  //' | cut -d: -f1 | sort -u | tr '\n' ';')"
    else
        echo "MISSED   $name  (wanted layer $layer to fail, exit=$status)"
        grep -E '^(PASS|FAIL)' <<<"$out" | sed 's/^/           /'
        missed=$((missed + 1))
    fi
    restore
}

dep() { sed -i "/^\[dependencies\]/a $1" "$CORE/Cargo.toml"; }

echo "== positive control"
if ./ci/gate.sh >/dev/null 2>&1; then echo "clean tree passes"; else echo "clean tree FAILS: fix that first"; exit 2; fi

echo "== violations"

echo 'extern crate std;' >>"$CORE/src/lib.rs"
expect "use of std in a gated crate (extern crate std)" "2\\."

dep 'glob = "0.3"'
echo 'pub fn g() { let _ = glob::glob("*"); }' >>"$CORE/src/lib.rs"
expect "std-only I/O dependency (glob)" "2\\."

dep 'libc = { version = "0.2", default-features = false }'
expect "I/O dependency that still compiles no_std (libc, declared but unused)" "3\\."

printf 'fn main() { println!("cargo:rustc-env=X=1"); }\n' >"$CORE/build.rs"
expect "own build script in a gated crate" "1\\."

mkdir -p "$EVIL/src"
printf '[package]\nname = "evil"\nversion = "0.1.0"\nedition = "2021"\n' >"$EVIL/Cargo.toml"
echo '#![no_std]' >"$EVIL/src/lib.rs"
echo 'fn main() {}' >"$EVIL/build.rs"
dep "evil = { path = \"$EVIL\" }"
sed -i 's/{ crate = "libharness" },/{ crate = "libharness" }, { crate = "evil" },/' deny.toml
expect "dependency with a build script (name allowlisted, its build script is not)" "3\\."

dep 'option-ext = "0.2"'
sed -i 's/{ crate = "libharness" },/{ crate = "libharness" }, { crate = "option-ext" },/' deny.toml
expect "dependency with a non-permissive licence (option-ext, MPL-2.0; name allowlisted)" "3\\."

cat >>"$CORE/src/lib.rs" <<'RS'
/// Violation: a spin guard held across an await point.
pub async fn held_across_await(c: &Counter) {
    let guard = c.0.lock();
    core::future::ready(()).await;
    drop(guard);
}
RS
expect "spin guard held across an await" "4\\."

echo 'pub fn u() -> u8 { unsafe { core::mem::zeroed() } }' >>"$CORE/src/lib.rs"
expect "unsafe code in a gated crate (forbid attribute present)" "2\\."

sed -i '/#!\[forbid(unsafe_code)\]/d' "$CORE/src/lib.rs"
expect "forbid(unsafe_code) removed from a gated crate" "1\\."

sed -i '/^#!\[no_std\]/d' "$CORE/src/lib.rs"
expect "no_std removed from a gated crate" "1\\."

python3 -I - "$CORE/Cargo.toml" <<'PY'
import re, sys
p = sys.argv[1]
s = open(p).read()
open(p, "w").write(re.sub(r"\[package\.metadata\.libharness\]\ngated = true\n", "", s))
PY
expect "gated crate loses its gated declaration" "1\\."

python3 -I - "$PROBE/Cargo.toml" <<'PY'
import re, sys
p = sys.argv[1]
s = open(p).read()
open(p, "w").write(re.sub(r"\[package\.metadata\.libharness\]\ngated = false\n", "", s))
PY
expect "ungated crate declares neither true nor false" "1\\."

cat >>"$PROBE/src/lib.rs" <<'RS'
unsafe extern "C" {
    fn host_write(fd: i32);
}
#[unsafe(no_mangle)]
pub extern "C" fn leak() {
    unsafe { host_write(1) }
}
RS
expect "probe with a wasm import" "5\\."

echo "== $((total - missed)) of $total violations caught"
[ "$missed" -eq 0 ]
