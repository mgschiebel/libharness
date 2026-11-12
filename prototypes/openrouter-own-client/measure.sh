#!/usr/bin/env bash
# Dependency numbers per transport variant. Usage: ./measure.sh   (needs python3; no network after `cargo fetch`)
# Counts unique host-target packages from `cargo tree -e normal`, excluding the workspace's own crates, and
# the subset that have a build.rs (custom-build target in `cargo metadata`).
set -euo pipefail
cd "$(dirname "$0")"
cargo metadata --format-version 1 > /tmp/own-client-meta.json
python3 -I - "$@" <<'PY'
import json, subprocess, sys
meta = json.load(open('/tmp/own-client-meta.json'))
ws = {p['name'] for p in meta['packages'] if p['source'] is None}
has_bs = {(p['name'], p['version']) for p in meta['packages'] if any('custom-build' in t['kind'] for t in p['targets'])}
variants = [
    ('adapter + codec only (no transport)', ['-p', 'provider-adapter']),
    ('codec only', ['-p', 'provider-codec']),
    ('std sockets (c)', ['-p', 'transport-std']),
    ('ureq 3, no TLS (a)', ['-p', 'transport-ureq', '--no-default-features']),
    ('ureq 3, rustls+ring (a, default)', ['-p', 'transport-ureq']),
    ('reqwest 0.13, no TLS (b)', ['-p', 'transport-reqwest', '--no-default-features']),
    ('reqwest 0.13, rustls (b, default)', ['-p', 'transport-reqwest']),
    ('reqwest 0.13, rustls + owned-runtime workaround', ['-p', 'transport-reqwest', '--features', 'owned-runtime']),
]
print('| variant | external packages | with build.rs | build.rs crates |')
print('|---|---|---|---|')
for name, args in variants:
    out = subprocess.run(['cargo', 'tree', '-e', 'normal', '--prefix', 'none', '--target', 'x86_64-unknown-linux-gnu', *args], capture_output=True, text=True, check=True).stdout
    pk = set()
    for l in out.splitlines():
        parts = l.replace(' (*)', '').replace(' (proc-macro)', '').split()
        if len(parts) >= 2 and parts[1].startswith('v') and parts[0] not in ws:
            pk.add((parts[0], parts[1][1:]))
    bs = sorted(n for n, v in pk if (n, v) in has_bs)
    print(f'| {name} | {len(pk)} | {len(bs)} | {", ".join(bs)} |')
PY
