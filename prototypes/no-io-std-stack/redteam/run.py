#!/usr/bin/env python3
"""PROTOTYPE red-team runner. Usage: run.py [case-id-prefix ...]
For each case: fresh copy of the baseline workspace + the escape, then run each layer ALONE:
  forbid = `cargo check`   (rustc only; clippy:: lints are inert)   caught iff `unsafe_code` diagnostic
  clippy = `cargo clippy -- -D warnings` (denylist; also runs rustc)             caught iff a disallowed_* diagnostic
  deny   = `cargo deny check bans sources`                           caught iff a deny diagnostic
  all    = scripts/check.sh (the real pipeline)                      caught iff it exits non-zero
  off    = sanity: builds with lints capped (the escape is real code that compiles)
Results -> redteam/results.json
"""
import json, os, re, shutil, subprocess, sys, time
HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
WORK = os.path.join(HERE, "work")
sys.path.insert(0, HERE)
from cases import CASES

SIG = {
    "forbid": re.compile(r"unsafe_code|usage of an `unsafe`|unsafe extern|call to unsafe"),
    "clippy": re.compile(r"disallowed[ _-](method|type|macro)"),
    "deny": re.compile(r"not-allowed|not explicitly allowed|banned|build-script-not-allowed|unmatched-source|unknown-registry|unknown-git"),
}
CMD = {
    "forbid": ["cargo", "check", "--workspace", "--message-format", "short"],
    "clippy": ["cargo", "clippy", "--workspace", "--message-format", "short", "--", "-D", "warnings"],
    "deny": ["cargo", "deny", "check", "bans", "sources"],
}

def run(cmd, cwd, env=None, timeout=300):
    e = dict(os.environ, CARGO_TARGET_DIR=os.path.join(WORK, "target"), CARGO_TERM_COLOR="never")
    e.update(env or {})
    t = time.time()
    p = subprocess.run(cmd, cwd=cwd, env=e, capture_output=True, text=True, timeout=timeout)
    return p.returncode, p.stdout + p.stderr, time.time() - t

def build(cid, c):
    d = os.path.join(WORK, cid)
    shutil.rmtree(d, ignore_errors=True)
    ws = os.path.join(d, "ws")
    os.makedirs(ws)
    for f in ("Cargo.toml", "Cargo.lock", "deny.toml", "clippy.toml"):
        shutil.copy(os.path.join(ROOT, f), ws)
    shutil.copytree(os.path.join(ROOT, "core"), os.path.join(ws, "core"))
    for name, files in c["deps"].items():
        for rel, txt in files.items():
            p = os.path.join(d, "deps", name, rel); os.makedirs(os.path.dirname(p), exist_ok=True); open(p, "w").write(txt)
    for rel, txt in c["files"].items():
        p = os.path.join(d, rel) if rel.startswith("ws/") else os.path.join(ws, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True); open(p, "w").write(txt)
    for rel, old, new in c["replace"]:
        p = os.path.join(ws, rel); t = open(p).read(); assert old in t, (cid, rel, old); open(p, "w").write(t.replace(old, new, 1))
    if c["lib"]:
        with open(os.path.join(ws, "core/src/lib.rs"), "a") as f: f.write(c["lib"])
    dp = os.path.join(ws, "deny.toml"); t = open(dp).read()
    for n in c["allow"]: t = t.replace("allow = [\n", f'allow = [\n    {{ crate = "{n}" }},\n', 1)
    for n in c["allow_build"]: t = t.replace("allow-build-scripts = [\n", f'allow-build-scripts = [\n    {{ name = "{n}" }},\n', 1)
    open(dp, "w").write(t)
    return ws

def main():
    sel = sys.argv[1:]
    res = {}
    for cid, c in CASES.items():
        if sel and not any(cid.startswith(s) for s in sel): continue
        ws = build(cid, c)
        r = {"title": c["title"], "group": c["group"], "note": c["note"]}
        rc, out, _ = run(["cargo", "build", "--workspace"], ws, {"RUSTFLAGS": "--cap-lints=allow"})
        r["compiles_unenforced"] = rc == 0
        if rc != 0: r["off_err"] = out[-600:]
        for layer in ("forbid", "clippy", "deny"):
            rc, out, dt = run(CMD[layer], ws)
            caught = rc != 0 and bool(SIG[layer].search(out))
            r[layer] = caught
            r[layer + "_rc"] = rc
            if rc != 0 and not caught: r[layer + "_other_err"] = out[-300:]
            if caught: r[layer + "_msg"] = next((l for l in out.splitlines() if SIG[layer].search(l)), "")[:160]
        rc, out, _ = run(["bash", os.path.join(ROOT, "scripts/check.sh")], ws)
        r["all"] = rc != 0
        r["all_union_agrees"] = r["all"] == (r["forbid"] or r["clippy"] or r["deny"])
        res[cid] = r
        print(f'{cid:34} off-builds={r["compiles_unenforced"]!s:5} forbid={r["forbid"]!s:5} clippy={r["clippy"]!s:5} deny={r["deny"]!s:5} all={r["all"]!s:5}', flush=True)
    path = os.path.join(HERE, "results.json")
    old = json.load(open(path)) if os.path.exists(path) and sel else {}
    old.update(res)
    json.dump(old, open(path, "w"), indent=1)

main()
