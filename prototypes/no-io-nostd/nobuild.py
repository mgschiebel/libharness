"""Fail if any workspace member has a build script (cargo-deny's allow-build-scripts does not cover workspace members)."""
import json, subprocess, sys
m = json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1"]))
bad = [f"{p['name']}: {t['src_path']}" for p in m["packages"] for t in p["targets"] if "custom-build" in t["kind"]]
print("\n".join(bad) or "no workspace build scripts")
sys.exit(1 if bad else 0)
