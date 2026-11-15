"""Workspace-crate checks for the no-I/O gate (ADR 0002). Run with `python3 -I ci/crates.py <mode>`.

Modes:
  check    fail unless every workspace crate declares `gated = true` or `gated = false` in
           `[package.metadata.libharness]`, and every gated crate meets the source-level rules below
  gated    print the names of the gated crates, one per line
  ungated  print the names of the ungated crates, one per line

Source-level rules for a gated crate (cargo-deny does not cover workspace members, so we do):
  * no build script of its own (`build.rs` or a `build = ` key),
  * its library root carries `#![no_std]` and `#![forbid(unsafe_code)]`.
"""
import json
import re
import subprocess
import sys


def packages():
    out = subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1"])
    return json.loads(out)["packages"]


def declared(pkg):
    """True, False, or None when the crate declares nothing usable."""
    meta = (pkg.get("metadata") or {}).get("libharness") or {}
    gated = meta.get("gated")
    return gated if isinstance(gated, bool) else None


def code_lines(path):
    with open(path, encoding="utf-8") as f:
        return [line for line in f.read().splitlines() if not line.lstrip().startswith("//")]


def problems(pkg):
    found = []
    gated = declared(pkg)
    if gated is None:
        found.append(f"{pkg['name']}: declares neither `gated = true` nor `gated = false` in [package.metadata.libharness]")
        return found
    if not gated:
        return found
    for target in pkg["targets"]:
        if "custom-build" in target["kind"]:
            found.append(f"{pkg['name']}: gated crate has its own build script ({target['src_path']})")
    libs = [t for t in pkg["targets"] if any(k in ("lib", "rlib") for k in t["kind"])]
    if not libs:
        found.append(f"{pkg['name']}: gated crate has no library target")
    for lib in libs:
        lines = code_lines(lib["src_path"])
        for attr, pattern in (("#![no_std]", r"^\s*#!\[no_std\]"), ("#![forbid(unsafe_code)]", r"^\s*#!\[forbid\(unsafe_code\)\]")):
            if not any(re.match(pattern, line) for line in lines):
                found.append(f"{pkg['name']}: library root {lib['src_path']} is missing {attr}")
    return found


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else "check"
    pkgs = packages()
    if mode == "gated":
        print("\n".join(p["name"] for p in pkgs if declared(p) is True))
    elif mode == "ungated":
        print("\n".join(p["name"] for p in pkgs if declared(p) is False))
    elif mode == "check":
        found = [msg for p in pkgs for msg in problems(p)]
        gated = [p["name"] for p in pkgs if declared(p) is True]
        if not gated:
            found.append("no gated crate in the workspace")
        print("\n".join(found) if found else f"ok: {len(pkgs)} crates classified, gated = {', '.join(gated)}")
        sys.exit(1 if found else 0)
    else:
        sys.exit(f"unknown mode {mode!r}")


main()
