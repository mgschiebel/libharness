#!/usr/bin/env bash
# Cold debug build per variant, each in a fresh target dir, registry already downloaded.
# Wall / user / sys seconds. Timings are noisy when other builds share the machine; load average is printed.
cd "$(dirname "$0")"
cargo fetch -q
run() { # name, args...
  local name=$1; shift
  local dir; dir=$(mktemp -d /tmp/cold-XXXXXX)
  echo "load before: $(cut -d' ' -f1-3 /proc/loadavg)"
  local s=$(date +%s.%N)
  local t; t=$( { TIMEFORMAT='%U %S'; time CARGO_TARGET_DIR=$dir cargo build -q "$@" ; } 2>&1 | tail -1)
  local e=$(date +%s.%N)
  printf '%s | wall %.1fs | user+sys %s | load after %s\n' "$name" "$(echo "$e - $s" | bc)" "$t" "$(cut -d' ' -f1 /proc/loadavg)"
  rm -rf "$dir"
}
run "adapter+codec (no transport)" -p provider-adapter
run "std sockets (c)" -p transport-std
run "ureq no TLS (a)" -p transport-ureq --no-default-features
run "ureq rustls+ring (a)" -p transport-ureq
run "reqwest no TLS (b)" -p transport-reqwest --no-default-features
run "reqwest rustls (b)" -p transport-reqwest
