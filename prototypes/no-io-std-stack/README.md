# Prototype: std enforcement stack (issue #23)

PROTOTYPE, throwaway. Branch `prototype/no-io-std-stack`, not for main. Answers: if the core keeps `std`, do `forbid(unsafe_code)` + clippy `disallowed_*` + a cargo-deny allowlist catch enough no-I/O violations, and what does it cost to keep the denylist complete? (Candidate 2 of `research/no-io-enforcement.md`, sections 4 and 14.) The session model is out of scope (issue #19); `core/` is a stub engine with serde + serde_json, the same dependency set as `prototype/session-task`.

## Run it

```
rustup toolchain install nightly --profile minimal -c rust-docs-json   # only for generation
cargo install cargo-deny --locked
python3 scripts/gen_denylist.py            # rustdoc JSON -> clippy.toml (3-6 s)
scripts/check.sh                           # the 3 layers on the real core crate
python3 redteam/run.py [id-prefix]         # red-team matrix -> redteam/results.json (~1 min, 450 MB in redteam/work, gitignored)
python3 redteam/report.py                  # markdown table
```

Toolchains: stable rustc/clippy 1.97.0 enforce; nightly 1.101.0 (2026-10-31) only produces the rustdoc JSON; cargo-deny 0.20.2.

## The stack

1. `[workspace.lints]`: `unsafe_code`, `clippy::disallowed_{methods,types,macros}` all at `forbid` (not `deny`, so `#[allow]` is a hard error).
2. `clippy.toml` at the workspace root, **generated**. `scripts/gen_denylist.py` walks the rustdoc JSON of `std` (and `core` for builtin macros), following `pub use` and glob re-exports, skipping `#[unstable]` items, and emits every public function, type and inherent method. Only a small `POLICY` table is hand-written (which modules/types are I/O: all of `fs`, `process`, `env`; `Instant`/`SystemTime`; `TcpStream`/`TcpListener`/`UdpSocket`; `stdin/stdout/stderr`; thread fns; macros `print*`, `eprint*`, `dbg`, `env`, `option_env`, `include*`).
3. `deny.toml`: `bans.allow` allowlist, `bans.build.allow-build-scripts`, `executables/interpreted = deny`, `wildcards = deny`, sources limited to crates.io.
4. CI step order in `scripts/check.sh`: `cargo check`, `cargo clippy -- -D warnings`, `cargo deny check bans sources`.

## Red-team matrix (38 cases, `redteam/cases.py`)

Each case is a fresh copy of the baseline workspace plus one escape. Every layer is run alone (`forbid` = `cargo check`, `clippy` = `cargo clippy -- -D warnings`, `deny` = `cargo deny check bans sources`), then the real pipeline (`all`). "Caught" needs a non-zero exit **and** a diagnostic matching that layer's signature (so a broken case cannot count as caught). Each case also builds with lints capped, proving the escape is real code; pipeline result equalled the union of the layers in all 38. Cases k01-k09 are controls (listed items).

| id | escape | forbid | clippy | deny | all |
|---|---|---|---|---|---|
| k01 | fs::read_to_string (listed) | missed | caught | missed | **caught** |
| k02 | TcpStream::connect (listed) | missed | caught | missed | **caught** |
| k03 | Command::new (listed) | missed | caught | missed | **caught** |
| k04 | env::var (listed) | missed | caught | missed | **caught** |
| k05 | Instant::now (listed) | missed | caught | missed | **caught** |
| k06 | thread::spawn (listed) | missed | caught | missed | **caught** |
| k07 | io::stdin().read_line (listed) | missed | caught | missed | **caught** |
| k08 | println! (listed macro) | missed | caught | missed | **caught** |
| k09 | SystemTime::now (listed) | missed | caught | missed | **caught** |
| e01 | unlisted std item: os::unix::net::UnixStream::connect | missed | missed | missed | **missed** |
| e02 | unlisted std item: Path::exists / canonicalize (std::path) | missed | missed | missed | **missed** |
| e03 | OpenOptions::new().open() | missed | caught | missed | **caught** |
| e04 | fs::read_dir | missed | caught | missed | **caught** |
| e05 | use std::fs as f; f::read_to_string | missed | caught | missed | **caught** |
| e06 | listed fn stored in a local fn pointer | missed | caught | missed | **caught** |
| e10 | std::fs inside a dependency NOT on the allowlist | missed | missed | caught | **caught** |
| e11 | std::fs inside a dependency a reviewer put on the allowlist | missed | missed | missed | **missed** |
| e12 | own crate calls libc::write in `unsafe` (libc allowlisted) | caught | missed | missed | **caught** |
| e13 | own `unsafe extern "C"` block, no libc | caught | missed | missed | **caught** |
| e14 | safe wrapper over FFI in an allowlisted dependency | missed | missed | missed | **missed** |
| e20 | build.rs in the core crate doing fs I/O | missed | caught | missed | **caught** |
| e21 | dependency with build.rs, name allowlisted, no build-script permission | missed | missed | caught | **caught** |
| e22 | dependency with build.rs, name AND build script allowlisted | missed | missed | missed | **missed** |
| e30 | proc macro reading files at expansion, not allowlisted | missed | missed | caught | **caught** |
| e31 | proc macro reading files at expansion, allowlisted by name | missed | missed | missed | **missed** |
| e40 | include_str!("/etc/hostname") | missed | caught | missed | **caught** |
| e41 | include_bytes!("/etc/hostname") | missed | caught | missed | **caught** |
| e42 | env!("PATH") (compile-time env read) | missed | caught | missed | **caught** |
| e43 | option_env!("HOME") | missed | caught | missed | **caught** |
| e50 | #[allow(clippy::disallowed_methods)] with lint level `deny` | missed | missed | missed | **missed** |
| e51 | #[allow(clippy::disallowed_methods)] with lint level `forbid` (baseline) | missed | caught | missed | **caught** |
| e52 | #[allow(unsafe_code)] + unsafe extern call | caught | missed | missed | **caught** |
| e53 | #[cfg_attr(all(), allow(clippy::disallowed_methods))] | missed | caught | missed | **caught** |
| e60 | contributor empties clippy.toml | missed | missed | missed | **missed** |
| e61 | contributor deletes `[lints] workspace = true` | missed | caught | missed | **caught** |
| e70 | macro_rules! exported by allowlisted dep expands to std::fs call | missed | caught | missed | **caught** |
| e71 | contributor adds a workspace member crate doing fs I/O | missed | caught | missed | **caught** |
| e72 | std::fs in core/tests/ (not part of the shipped lib) | missed | missed | missed | **missed** |

Reading: `e10`, `e30`, `e21` are the "same escape, one layer later" pairs: an unlisted dependency is caught by deny; once a reviewer allowlists it (`e11`, `e31`, `e22`) nothing catches it. `e50` is the baseline escape with `disallowed_methods` set to `deny` instead of `forbid`: `#[allow]` then wins, which is why the baseline uses `forbid`. `e72` (test targets do not ship) is missed by design.

### Which escape is closed by which layer

| Escape | Closed by | Left open |
|---|---|---|
| Listed std item (fs, net, process, env, time, thread, stdio, print macros), incl. alias and fn pointer | clippy (generated list) | |
| `OpenOptions::open`, `read_dir` (missed by the hand-written list in research section 4) | clippy, because the generator lists them | |
| `include_str!`, `include_bytes!`, `env!`, `option_env!` | clippy `disallowed_macros` | |
| `#[allow(...)]`, `cfg_attr(allow)` for clippy and for `unsafe_code` | `forbid` level (rustc E0453) | only if someone lowers the level (`e50`) |
| Own `unsafe`, `extern "C"`, `libc::*` calls | `forbid(unsafe_code)` | |
| Own `build.rs` | clippy (it lints build scripts of workspace members) | |
| Macro from a dependency expanding to a listed call | clippy (expansion is linted in our crate) | |
| New workspace member / removed `[lints]` table | clippy via root `clippy.toml` + `-D warnings` | member still lacks `forbid(unsafe_code)` unless it has `[lints] workspace = true` |
| Unlisted dependency, proc macro, build script | cargo-deny allowlist + `allow-build-scripts` | |
| **Unlisted std item** (`os::unix::net`, `Path::exists`, ...) | nothing | outside the generated modules; closed in a `--wide` run (see below) but the module list is hand-picked |
| **Std I/O inside an allowlisted dependency** | nothing | R3: reviewer judgement |
| **Safe FFI wrapper in an allowlisted dependency** | nothing | R2/R3: reviewer judgement |
| **Allowlisted proc macro / build script doing I/O at build time** | nothing | R5: reviewer judgement |
| **Editing `clippy.toml` / `deny.toml` / lint levels** | nothing (tools trust their config) | CODEOWNERS on those files |
| I/O in `tests/` | nothing | by design out of scope |

Missed with all layers on: **9 of 38 cases** (9 of the 29 non-control escapes; 8 under the baseline config, the 9th being the weakened `deny` level). Of the 8: 2 are unlisted std items (fixable by widening the list), 4 are "allowlisted dependency does I/O" (R3/R5), 1 is config tampering, 1 is test code.

## Measurements

| Measure | Result |
|---|---|
| Generated denylist | 275 entries (221 methods, 44 types, 10 macros); 283 lines, 27 KB. `--wide` (adds `std::os::unix::net` and 10 `Path` methods): 335 entries |
| False positives, legitimate pure code | 0 on `core/src/corpus.rs` (collections, fmt, `io::Cursor`/`Read`/`Write` on memory, `io::copy`, `Path` joins, `IpAddr`/`SocketAddr`, `Duration`, `mpsc`, `Mutex`, `thread_local!`, `env::consts`). Probe of 8 borderline lines: 7 flagged (`thread::current`, `process::id`, `ExitCode`, `available_parallelism`, an injected `Instant` used for arithmetic, `UNIX_EPOCH`, `VarError`); only `&mut dyn io::Write` passed. Meaning: any type whose name is in the list is banned even when used inertly |
| Time to regenerate | 3.4-3.5 s warm (std.json 18 MB + core.json 97 MB, python3 stdlib only); toolchain install is the real cost |
| Churn between toolchains | nightly 1.99 (2026-07-31) vs 1.101 (2026-10-31): +2 entries (`fs::set_times`, `fs::set_times_nofollow`). Rerun and diff on every toolchain bump |
| Missed escapes, all layers on | 9/38 (8/38 with baseline config); see above |
| Allowlist size, same deps as session-task | 11 crates (itoa, memchr, proc-macro2, quote, serde, serde_core, serde_derive, serde_json, syn, unicode-ident, zmij) plus 7 `allow-build-scripts` entries (serde, serde_core, serde_json, proc-macro2, zmij, memchr, quote). Every added crate costs a name entry, possibly a build-script entry, and a human review |
| CI time cost | clippy incremental 0.17 s with the denylist vs 0.15 s without; `cargo check` 0.1 s; `cargo deny check bans sources` 0.12 s |
| Reviewer checks by hand | see below |

## What a reviewer must still check by hand

- Every new allowlist entry: does the crate (and its build script / proc macro) do I/O or call FFI, now or after an update? Names only are matched; versions can change behaviour.
- Every `allow-build-scripts` entry.
- Any PR touching `clippy.toml`, `deny.toml`, `[workspace.lints]`, `Cargo.toml` `[lints]`, `scripts/check.sh`, or the `POLICY` table (CODEOWNERS).
- Whether std modules outside the generator's module list gained I/O surface (`os::*`, `path`, `sys`-like items) on a toolchain bump.
- Semantics of safe wrappers in allowlisted dependencies (nothing automated looks inside them).

## Surprises

- The rustdoc JSON lists nightly-only items; putting them in `clippy.toml` made stable clippy warn 50+ times ("does not refer to a reachable function"). The generator must filter on the `stability` field.
- Builtin macros (`include_str!`, `option_env!`...) are not in `std.json` at all; they are in `core.json` (97 MB). `std::include_str` as a `disallowed_macros` path still resolves in clippy 0.1.97, and so does `std::env` (macro) next to the module `std::env`.
- Many public paths are glob re-exports (`std::os::unix::net` is `pub use self::stream::*`); a naive walk finds nothing there.
- `clippy.toml` at the workspace root is found by member crates (ancestors are searched), so one file covers new members. Clippy also lints `build.rs` of workspace members and macro expansions from dependency `macro_rules!`.
- Without `-D warnings`, deleting a member's `[lints]` table downgrades the disallowed lints to warnings (rc 0, verified). The CI `-D warnings` is a load-bearing backstop.
- cargo-deny treats a path dependency without `version` as a wildcard; test deps needed `version = "0.1"`.
- Clippy `disallowed_types` flags inert uses too (`Instant` as a parameter, `UNIX_EPOCH`), a sharper edge than disallowing only constructors.

## Assumptions (no human was available)

- "Same dependency set as the no_std prototype": `prototype/session-task` (serde, serde_json) was used; no separate no_std prototype branch was fetched.
- Trait methods of `io::Read`/`io::Write` are deliberately not banned (they work on `Vec<u8>`/`Cursor` and injected writers). Anything reaching a real file or socket needs a listed constructor first.
- Policy choices (all of `process`, `thread` functions but not `ThreadId`, `Instant`/`SystemTime` but not `Duration`) are my judgement; they live in `POLICY`.
- Only `bans` and `sources` of cargo-deny are used (advisories/licenses are unrelated and need a database fetch). Linux/x86_64 only.
- Escapes read `/etc/hostname`; nothing was run that mutates the host. `build.rs` and proc-macro cases do execute at build time by design.
- The red-team counts "caught" by diagnostic signature plus exit code; one pipeline run per case.

## Recommendation for #24 (non-binding)

The stack is cheap (one generated file, one `deny.toml`, ~0.2 s of CI) and closes everything in the own crate that is expressible as a std path or macro, with 0 false positives on ordinary pure code. It leaves open exactly the R3/R5 class (an allowlisted dependency, proc macro or build script that does I/O) and config tampering, which are human-review problems in every `std` design, and it needs a regeneration step plus a diff review on every toolchain bump. If `std` collections/`Mutex` matter more than a compile-time guarantee, this is an acceptable fallback; if the no_std gate (candidate 1) turns out affordable, it additionally covers whole-tree R1 that this stack never sees (a dependency's own `std::fs` call, case `e11`). Pair either with CODEOWNERS on the config files.
