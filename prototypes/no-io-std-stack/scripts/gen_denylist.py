#!/usr/bin/env python3
"""PROTOTYPE. Generate clippy.toml disallowed-{methods,types,macros} from rustdoc JSON of std.

Usage: gen_denylist.py [--toolchain nightly] [--json PATH] [--out clippy.toml] [--wide]
Default json: nightly sysroot share/doc/rust/json/std.json
(needs: rustup toolchain install nightly --profile minimal -c rust-docs-json)
Only the POLICY table below is hand-written: which module/type is I/O and which is pure.
"""
import json, subprocess, sys, argparse, os

# policy per module: 'all' = every public fn/type/inherent method; otherwise explicit
POLICY = {
    "std::fs":      {"all": True},
    "std::process": {"all": True},
    "std::env":     {"all": True, "skip_modules": ["consts"]},
    "std::thread":  {"fns": "all", "types": ["Builder", "JoinHandle", "Scope", "ScopedJoinHandle"]},
    "std::time":    {"types": ["Instant", "SystemTime"]},
    "std::net":     {"types": ["TcpStream", "TcpListener", "UdpSocket", "Incoming"],
                     "trait_methods": ["ToSocketAddrs::to_socket_addrs"]},
    "std::io":      {"fns": ["stdin", "stdout", "stderr"],
                     "types": ["Stdin", "StdinLock", "Stdout", "StdoutLock", "Stderr", "StderrLock"],
                     "trait_methods": ["IsTerminal::is_terminal"]},
}
# extra modules for the "wider" run (--wide): os-level escape hatches
WIDE = {
    "std::os::unix::net": {"all": True},
    "std::path": {"types_methods": {"Path": ["exists", "try_exists", "metadata", "symlink_metadata",
                  "canonicalize", "read_link", "read_dir", "is_file", "is_dir", "is_symlink"],
                  "PathBuf": []}},
}
MACROS = ["print", "println", "eprint", "eprintln", "dbg",
          "env", "option_env", "include", "include_str", "include_bytes"]

def load(path):
    d = json.load(open(path))
    return d, {int(k): v for k, v in d["index"].items()}

def children(I, mod):
    """yield (name, item) for public items of a module, following `use` re-exports."""
    for cid in mod["inner"]["module"]["items"]:
        it = I.get(cid)
        if not it or it.get("visibility") not in ("public", "default"):
            continue
        if (it.get("stability") or {}).get("level") == "unstable":
            continue  # nightly-only: clippy on stable warns "does not refer to a reachable function"
        inner = it["inner"]
        if "use" in inner:
            u = inner["use"]
            tgt = I.get(u["id"]) if u.get("id") is not None else None
            if u.get("is_glob"):  # `pub use self::m::*;` re-exports the children of m
                if tgt is not None and "module" in tgt["inner"]:
                    yield from children(I, tgt)
                continue
            if tgt is not None:
                yield u["name"], tgt
        else:
            yield it["name"], it

def inherent_methods(I, ty):
    kind = next(k for k in ("struct", "enum", "union") if k in ty["inner"])
    for iid in ty["inner"][kind].get("impls", []):
        imp = I.get(iid)
        if not imp or "impl" not in imp["inner"] or imp["inner"]["impl"].get("trait"):
            continue
        for mid in imp["inner"]["impl"]["items"]:
            m = I.get(mid)
            if (m and "function" in m["inner"] and m.get("visibility") == "public"
                    and (m.get("stability") or {}).get("level") != "unstable"):
                yield m["name"]

def walk(d, I, root_path, pol, out, depth=0):
    mod = I[find_mod(d, I, root_path)]
    for name, it in children(I, mod):
        inner = it["inner"]
        path = f"{root_path}::{name}"
        if "module" in inner:
            if name in pol.get("skip_modules", []):
                continue
            if pol.get("all") and depth < 3:
                walk(d, I, path, pol, out, depth + 1)
            continue
        if "function" in inner:
            if pol.get("all") or pol.get("fns") == "all" or name in pol.get("fns", []):
                out["methods"].add(path)
        elif any(k in inner for k in ("struct", "enum", "union")):
            if pol.get("all") or name in pol.get("types", []):
                out["types"].add(path)
                for m in inherent_methods(I, it):
                    out["methods"].add(f"{path}::{m}")
            for m in pol.get("types_methods", {}).get(name, []):
                out["methods"].add(f"{path}::{m}")
        elif "trait" in inner:
            for tm in pol.get("trait_methods", []):
                t, m = tm.split("::")
                if t == name:
                    out["methods"].add(f"{path}::{m}")

def find_mod(d, I, path):
    parts = path.split("::")
    cur = d["root"]
    for p in parts[1:]:
        for name, it in children(I, I[cur]):
            if name == p and "module" in it["inner"]:
                cur = it["id"]; break
        else:
            raise SystemExit(f"module {path} not found")
    return cur

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--json")
    ap.add_argument("--toolchain", default="nightly")
    ap.add_argument("--out", default="clippy.toml")
    ap.add_argument("--wide", action="store_true")
    a = ap.parse_args()
    js = a.json
    if not js:
        sr = subprocess.check_output(["rustc", "+" + a.toolchain, "--print", "sysroot"], text=True).strip()
        js = f"{sr}/share/doc/rust/json/std.json"
    ver = subprocess.run(["rustc", "+" + a.toolchain, "-V"], capture_output=True, text=True).stdout.strip()
    d, I = load(js)
    pols = dict(POLICY)
    if a.wide:
        pols.update(WIDE)
    out = {"methods": set(), "types": set(), "macros": set()}
    for m, pol in pols.items():
        walk(d, I, m, pol, out)
    root = I[d["root"]]
    names = {n for n, _ in children(I, root)}
    for m in MACROS:
        out["macros"].add(f"std::{m}")
    # builtin macros (env!, include_str!, ...) are defined in core and re-exported by std
    core = json.load(open(js.replace("std.json", "core.json")))
    cnames = {v["name"] for v in core["index"].values() if "macro" in v["inner"]}
    miss = [m for m in MACROS if m not in names and m not in cnames]
    if miss:
        print("note: macros not found in std/core rustdoc JSON:", miss, file=sys.stderr)
    with open(a.out, "w") as f:
        f.write(f"# GENERATED by scripts/gen_denylist.py from {ver}, rustdoc JSON format {d['format_version']}.\n# Do not edit by hand.\n")
        for key, toml in (("methods", "disallowed-methods"), ("types", "disallowed-types"), ("macros", "disallowed-macros")):
            f.write(f"{toml} = [\n")
            for p in sorted(out[key]):
                f.write(f'    {{ path = "{p}", reason = "no-I/O core: inject a capability instead" }},\n')
            f.write("]\n")
    print({k: len(v) for k, v in out.items()}, "->", a.out)

main()
