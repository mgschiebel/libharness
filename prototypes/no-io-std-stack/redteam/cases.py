"""Red-team cases. Each case = an escape applied to a fresh copy of the baseline workspace.
Fields:
  title, group
  lib: text appended to core/src/lib.rs
  files: {relpath: content} written into the workspace (ws/...) ; deps live in ../deps/<name>
  replace: [(relpath, old, new)]
  allow: crates added to the cargo-deny allowlist (a reviewer 'approved' them)
  allow_build: crates added to allow-build-scripts
  note
"""
R = "/etc/hostname"
CASES = {}
def case(cid, title, group, lib="", files=None, replace=None, allow=None, allow_build=None, deps=None, note=""):
    CASES[cid] = dict(title=title, group=group, lib=lib, files=files or {}, replace=replace or [],
                      allow=allow or [], allow_build=allow_build or [], deps=deps or {}, note=note)

DEPTOML = lambda name, extra="": f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n{extra}\n'
def depedit(name):  # add path dep to core/Cargo.toml
    return ("core/Cargo.toml", 'serde_json = "1"\n', f'serde_json = "1"\n{name} = {{ path = "../../deps/{name}", version = "0.1" }}\n')

# ---- controls: items the generator lists (should be caught by clippy)
for cid, title, body in [
    ("k01-fs-read", "fs::read_to_string (listed)", f'std::fs::read_to_string("{R}").unwrap();'),
    ("k02-net", "TcpStream::connect (listed)", 'let _ = std::net::TcpStream::connect("127.0.0.1:9");'),
    ("k03-process", "Command::new (listed)", 'let _ = std::process::Command::new("true").status();'),
    ("k04-env", "env::var (listed)", 'let _ = std::env::var("HOME");'),
    ("k05-time", "Instant::now (listed)", 'let _ = std::time::Instant::now();'),
    ("k06-thread", "thread::spawn (listed)", 'let _ = std::thread::spawn(|| {});'),
    ("k07-stdin", "io::stdin().read_line (listed)", 'let mut s = String::new(); let _ = std::io::stdin().read_line(&mut s);'),
    ("k08-println", "println! (listed macro)", 'println!("leak");'),
    ("k09-systemtime", "SystemTime::now (listed)", 'let _ = std::time::SystemTime::now();'),
]:
    case(cid, title, "control: listed std item", lib=f"\npub fn esc() {{ {body} }}\n")

# ---- brief escapes
case("e01-unlisted-unix-socket", "unlisted std item: os::unix::net::UnixStream::connect", "unlisted std item",
     lib='\npub fn esc() { let _ = std::os::unix::net::UnixStream::connect("/tmp/x.sock"); }\n')
case("e02-unlisted-path-exists", "unlisted std item: Path::exists / canonicalize (std::path)", "unlisted std item",
     lib=f'\npub fn esc() {{ let p = std::path::Path::new("{R}"); let _ = (p.exists(), p.canonicalize()); }}\n')
case("e03-openoptions-open", "OpenOptions::new().open()", "OpenOptions::open",
     lib=f'\npub fn esc() {{ let _ = std::fs::OpenOptions::new().read(true).open("{R}"); }}\n')
case("e04-read-dir", "fs::read_dir", "read_dir", lib='\npub fn esc() { let _ = std::fs::read_dir("/"); }\n')
case("e05-renamed-import", "use std::fs as f; f::read_to_string", "listed item via alias",
     lib=f'\nuse std::fs as f;\npub fn esc() {{ let _ = f::read_to_string("{R}"); }}\n')
case("e06-fn-pointer", "listed fn stored in a local fn pointer", "listed item via alias",
     lib=f'\npub fn esc() {{ let g = std::fs::read_to_string::<&str>; let _ = g("{R}"); }}\n')

IO_DEP = f'pub fn slurp() -> String {{ std::fs::read_to_string("{R}").unwrap_or_default() }}\n'
case("e10-dep-io-not-allowlisted", "std::fs inside a dependency NOT on the allowlist", "std I/O inside a dependency",
     deps={"glob-lite": {"Cargo.toml": DEPTOML("glob-lite"), "src/lib.rs": IO_DEP.replace("slurp", "glob")}},
     replace=[depedit("glob-lite")], lib='\npub fn esc() { let _ = glob_lite::glob(); }\n')
case("e11-dep-io-allowlisted", "std::fs inside a dependency a reviewer put on the allowlist", "std I/O inside a dependency",
     deps={"tiny-util": {"Cargo.toml": DEPTOML("tiny-util"), "src/lib.rs": IO_DEP}}, allow=["tiny-util"],
     replace=[depedit("tiny-util")], lib='\npub fn esc() { let _ = tiny_util::slurp(); }\n')
case("e12-ffi-libc-own-unsafe", "own crate calls libc::write in `unsafe` (libc allowlisted)", "FFI through allowlisted libc",
     replace=[("core/Cargo.toml", 'serde_json = "1"\n', 'serde_json = "1"\nlibc = "0.2"\n')], allow=["libc"], allow_build=["libc"],
     lib='\npub fn esc() { unsafe { libc::write(1, b"x".as_ptr().cast(), 1); } }\n')
case("e13-ffi-extern-c", "own `unsafe extern \"C\"` block, no libc", "FFI through allowlisted libc",
     lib='\nunsafe extern "C" { fn write(fd: i32, b: *const u8, n: usize) -> isize; }\npub fn esc() { unsafe { write(1, b"x".as_ptr(), 1); } }\n')
case("e14-ffi-safe-wrapper-dep", "safe wrapper over FFI in an allowlisted dependency", "FFI through allowlisted libc",
     deps={"safe-sys": {"Cargo.toml": DEPTOML("safe-sys"),
       "src/lib.rs": 'unsafe extern "C" { fn write(fd: i32, b: *const u8, n: usize) -> isize; }\npub fn write_stdout(b: &[u8]) { unsafe { write(1, b.as_ptr(), b.len()); } }\n'}},
     allow=["safe-sys"], replace=[depedit("safe-sys")], lib='\npub fn esc() { safe_sys::write_stdout(b"x"); }\n')
BUILD_RS = f'fn main() {{ let _ = std::fs::read_to_string("{R}"); let _ = std::fs::write(std::env::var("OUT_DIR").unwrap() + "/x", "x"); }}\n'
case("e20-build-rs-own", "build.rs in the core crate doing fs I/O", "build.rs", files={"ws/core/build.rs": BUILD_RS})
case("e21-build-rs-dep-no-permission", "dependency with build.rs, name allowlisted, no build-script permission", "build.rs",
     deps={"tiny-build": {"Cargo.toml": DEPTOML("tiny-build"), "build.rs": BUILD_RS, "src/lib.rs": "pub fn x() {}\n"}},
     allow=["tiny-build"], replace=[depedit("tiny-build")], lib='\npub fn esc() { tiny_build::x(); }\n')
case("e22-build-rs-dep-allowed", "dependency with build.rs, name AND build script allowlisted", "build.rs",
     deps={"tiny-build": {"Cargo.toml": DEPTOML("tiny-build"), "build.rs": BUILD_RS, "src/lib.rs": "pub fn x() {}\n"}},
     allow=["tiny-build"], allow_build=["tiny-build"], replace=[depedit("tiny-build")], lib='\npub fn esc() { tiny_build::x(); }\n')
PM = lambda: {"Cargo.toml": DEPTOML("derive-peek", "[lib]\nproc-macro = true\n"),
  "src/lib.rs": f'use std::str::FromStr;\n#[proc_macro_derive(Peek)]\npub fn d(_i: proc_macro::TokenStream) -> proc_macro::TokenStream {{ let _ = std::fs::read_to_string("{R}"); proc_macro::TokenStream::from_str("").unwrap() }}\n'}
case("e30-proc-macro-not-allowlisted", "proc macro reading files at expansion, not allowlisted", "proc macro",
     deps={"derive-peek": PM()}, replace=[depedit("derive-peek")], lib='\n#[derive(derive_peek::Peek)]\npub struct S;\n')
case("e31-proc-macro-allowlisted", "proc macro reading files at expansion, allowlisted by name", "proc macro",
     deps={"derive-peek": PM()}, allow=["derive-peek"], replace=[depedit("derive-peek")], lib='\n#[derive(derive_peek::Peek)]\npub struct S;\n')
case("e40-include-str", "include_str!(\"/etc/hostname\")", "include_str!/env!", lib=f'\npub static H: &str = include_str!("{R}");\n')
case("e41-include-bytes", "include_bytes!(\"/etc/hostname\")", "include_str!/env!", lib=f'\npub static H: &[u8] = include_bytes!("{R}");\n')
case("e42-env-macro", "env!(\"PATH\") (compile-time env read)", "include_str!/env!", lib='\npub static H: &str = env!("PATH");\n')
case("e43-option-env-macro", "option_env!(\"HOME\")", "include_str!/env!", lib='\npub static H: Option<&str> = option_env!("HOME");\n')
FS = f'let _ = std::fs::read_to_string("{R}");'
case("e50-allow-attr-deny-level", "#[allow(clippy::disallowed_methods)] with lint level `deny`", "#[allow] by contributor",
     replace=[("Cargo.toml", 'disallowed_methods = "forbid"', 'disallowed_methods = "deny"')], lib=f'\n#[allow(clippy::disallowed_methods)]\npub fn esc() {{ {FS} }}\n',
     note="disallowed_methods level swapped from forbid to deny in workspace Cargo.toml")
case("e51-allow-attr-forbid-level", "#[allow(clippy::disallowed_methods)] with lint level `forbid` (baseline)", "#[allow] by contributor",
     lib=f'\n#[allow(clippy::disallowed_methods)]\npub fn esc() {{ {FS} }}\n')
case("e52-allow-unsafe-code", "#[allow(unsafe_code)] + unsafe extern call", "#[allow] by contributor",
     lib='\n#[allow(unsafe_code)]\npub fn esc() { unsafe extern "C" { fn getpid() -> i32; } let _ = unsafe { getpid() }; }\n')
case("e53-cfg-attr-allow", "#[cfg_attr(all(), allow(clippy::disallowed_methods))]", "#[allow] by contributor",
     lib=f'\n#[cfg_attr(all(), allow(clippy::disallowed_methods))]\npub fn esc() {{ {FS} }}\n')
case("e60-tamper-clippy-toml", "contributor empties clippy.toml", "config tampering",
     files={"ws/clippy.toml": "disallowed-methods = []\n"}, lib=f'\npub fn esc() {{ {FS} }}\n')
case("e61-tamper-lints-table", "contributor deletes `[lints] workspace = true`", "config tampering",
     replace=[("core/Cargo.toml", "[lints]\nworkspace = true\n", "")], lib=f'\npub fn esc() {{ {FS} }}\n')
case("e70-macro-from-allowlisted-dep", "macro_rules! exported by allowlisted dep expands to std::fs call", "std I/O inside a dependency",
     deps={"io-macros": {"Cargo.toml": DEPTOML("io-macros"),
       "src/lib.rs": f'#[macro_export]\nmacro_rules! slurp {{ () => {{ ::std::fs::read_to_string("{R}") }} }}\n'}},
     allow=["io-macros"], replace=[depedit("io-macros")], lib='\npub fn esc() { let _ = io_macros::slurp!(); }\n')
case("e71-new-workspace-member", "contributor adds a workspace member crate doing fs I/O", "workspace shape",
     files={"ws/extra/Cargo.toml": DEPTOML("extra"), "ws/extra/src/lib.rs": IO_DEP},
     replace=[("Cargo.toml", 'members = ["core"]', 'members = ["core", "extra"]')], note="extra has no [lints] table")
case("e72-test-target", "std::fs in core/tests/ (not part of the shipped lib)", "workspace shape",
     files={"ws/core/tests/t.rs": f'#[test] fn t() {{ {FS} }}\n'}, note="by design out of scope: tests do not ship")
