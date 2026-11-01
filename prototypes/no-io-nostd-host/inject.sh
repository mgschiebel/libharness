#!/usr/bin/env bash
# PROTOTYPE: inject each forbidden thing, run ci.sh, expect FAIL, restore.
cd "$(dirname "$0")"
backup() { cp core/Cargo.toml /tmp/c.toml; cp core/src/lib.rs /tmp/c.rs; cp probe/src/lib.rs /tmp/p.rs; cp Cargo.lock /tmp/lock; }
restore() { cp /tmp/deny.orig deny.toml; cp /tmp/c.toml core/Cargo.toml; cp /tmp/c.rs core/src/lib.rs; cp /tmp/p.rs probe/src/lib.rs; cp /tmp/lock Cargo.lock; rm -f core/build.rs; }
dep() { sed -i "/^\\[dependencies\\]/a $1" core/Cargo.toml; }
cp deny.toml /tmp/deny.orig
backup
run() { echo "=== INJECT: $1"; CI_TAIL=${CI_TAIL:-3} ./ci.sh 2>&1 | grep -vE '^\s*$'; echo "exit=${PIPESTATUS[0]}"; restore; }
dep 'glob = "0.3"'
echo 'pub fn g() { let _ = glob::glob("*"); }' >> core/src/lib.rs
run "std-only crate (glob)"
echo 'extern crate std;' >> core/src/lib.rs
run "extern crate std"
dep 'getrandom = { version = "0.3", default-features = false }'
echo 'pub fn r() { let mut b=[0u8;4]; let _ = getrandom::fill(&mut b); }' >> core/src/lib.rs
run "getrandom"
dep 'libc = { version = "0.2", default-features = false }'
echo 'pub fn l() -> i32 { 0 }' >> core/src/lib.rs
run "libc (declared, unused)"
echo 'unsafe extern "C" { fn write(fd: i32, p: *const u8, n: usize) -> isize; }' >> core/src/lib.rs
run "unsafe extern block in core (not called)"
echo 'unsafe extern "C" { fn host_write(p: i32); } pub fn w() { unsafe { host_write(1) } }' >> core/src/lib.rs
run "unsafe extern block in core + call"
printf 'fn main() { println!("cargo:rustc-env=X=1"); }\n' > core/build.rs
run "build script in core itself (workspace member)"
rm -rf /tmp/evil; mkdir -p /tmp/evil/src; printf '[package]\nname="evil"\nversion="0.1.0"\nedition="2021"\n' > /tmp/evil/Cargo.toml
echo '#![no_std]' > /tmp/evil/src/lib.rs; echo 'fn main(){}' > /tmp/evil/build.rs
cp deny.toml /tmp/deny.bak
dep 'evil = { path = "/tmp/evil" }'
sed -i 's/"nostd-core", "probe",/"nostd-core", "probe", "evil",/' deny.toml
run "dependency with a build script (name allowlisted, build not)"
cp /tmp/deny.bak deny.toml
