# Cross-platform release pipeline (macOS / Linux / Windows) — design

**Date:** 2026-08-10
**Status:** Approved

## Problem

`herdr plugin install talent-factory/herdr-linear` currently runs the manifest's only
`[[build]]` step as-is:

```toml
[[build]]
command = ["cargo", "build", "--release", "--features", "plugin"]
```

That's a full from-scratch `cargo build --release` of a 347-crate dependency tree
(`reqwest`, `tokio` `full`, `ratatui`, `crossterm`, `graphql_client`, …) under a
maximum-optimization release profile (`lto = true`, `codegen-units = 1`). Every install
or reinstall recompiles everything — no cache is reused across installs — which makes
install take minutes instead of seconds and requires a Rust toolchain on the installing
machine. The manifest's own header comment already flags this as a known v1 gap:
*"installing requires a Rust toolchain (no prebuilt-binary download yet)"*.

Separately, the plugin only declares `platforms = ["linux", "macos"]` — Windows was
explicitly deferred in the original plugin-layer design (2026-08-04), unlike the
`herdr-file-viewer` reference plugin, which already ships full Windows support.

## Scope

- A tag-triggered GitHub Actions release workflow that builds `herdr-linear` for 5
  targets and publishes checksummed binaries to a GitHub Release.
- A `[[build]]` step that downloads and verifies the matching prebuilt binary for the
  installing machine, falling back to `cargo build --release --features plugin` on any
  miss (unsupported platform, network failure, missing/mismatched checksum, no release
  for the declared version) — install must never get harder than it is today.
- Full Windows platform support: `platforms = ["linux", "macos", "windows"]`, with
  Windows-specific `[[actions]]` that work around a real herdr limitation (see
  "Windows pane-spawn limitation" below), mirroring the proven `herdr-file-viewer`
  pattern.
- A small `Cargo.toml` change (`reqwest` → `rustls-tls`) required to produce a
  statically-linked musl Linux binary.

**Out of scope for this design:** Linux/Windows aarch64 targets, binary signing /
notarization, a Homebrew/winget/etc. distribution channel, changes to the local dev
loop (`just plugin-reinstall` keeps using a direct `cargo build`).

## Targets

| OS | Target triple | Asset name |
|---|---|---|
| macOS (Apple Silicon) | `aarch64-apple-darwin` | `herdr-linear-aarch64-apple-darwin` |
| macOS (Intel) | `x86_64-apple-darwin` | `herdr-linear-x86_64-apple-darwin` |
| Linux | `x86_64-unknown-linux-musl` | `herdr-linear-x86_64-unknown-linux-musl` |
| Windows | `x86_64-pc-windows-msvc` | `herdr-linear-x86_64-pc-windows-msvc.exe` |

Linux uses **musl**, not glibc: `reqwest` currently pulls in `native-tls` (system
OpenSSL) by default, which doesn't cross-compile cleanly for a static musl target.
`Cargo.toml` switches it to `rustls-tls`:

```toml
reqwest = { version = "0.11", default-features = false, features = ["json", "rustls-tls"] }
```

This affects all platforms, not just Linux — every build now uses `rustls` instead of
the platform's native TLS stack (OpenSSL / Schannel / Security.framework). Functionally
equivalent for our HTTPS GraphQL calls to Linear; verified via `cargo test
--all-features` plus one manual live call during implementation.

## Architecture

### New files

- `.github/workflows/release.yml` — build matrix (5 targets) + publish job, triggered
  by `v*` tag pushes.
- `scripts/fetch-or-build.sh` — `[[build]]` step for macOS/Linux.
- `scripts/fetch-or-build.ps1` — `[[build]]` step for Windows.
- `scripts/open-split-windows.ps1`, `scripts/open-tab-windows.ps1` — Windows siblings
  of `scripts/open-split.sh` / `scripts/open-tab.sh`.
- `tests/manifest.rs`, `tests/launcher_content.rs`, `tests/fetch_or_build.rs`,
  `tests/release_workflow.rs` — structural content assertions (see Testing strategy).

### Changed files

- `herdr-plugin.toml` — `platforms` gains `"windows"`; `[[build]]` and the two panel
  `[[actions]]` become platform-gated (one Unix entry, one Windows entry each, distinct
  `id`s for the Windows ones). `min_herdr_version` stays `"0.7.0"` — platform-gated
  build/action items are already supported at that version (verified against
  `herdr-file-viewer`'s manifest, which declares the same floor).
- `Cargo.toml` — `reqwest` TLS backend switch (above).
- `README.md` — Windows section (which action ids to bind, known limitations),
  install note that a Rust toolchain is no longer required when a matching release
  exists.

### Unchanged

- `scripts/open-split.sh` / `open-tab.sh` — untouched; just gain an explicit
  `platforms = ["linux", "macos"]` on their manifest entries.
- `justfile`'s `plugin-reinstall` recipe — the local dev loop keeps building directly
  with `cargo build --release --features plugin`; it is not part of the release path.
- The Rust binary itself — `--launch-decision` / `--launch-decision-tab` already exist
  and are reused as-is by the new Windows scripts.

### `herdr-plugin.toml` shape

```toml
platforms = ["linux", "macos", "windows"]

[[build]]
platforms = ["linux", "macos"]
command = ["/bin/sh", "scripts/fetch-or-build.sh"]

[[build]]
platforms = ["windows"]
command = ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "scripts/fetch-or-build.ps1"]

[[panes]]
id = "linear-panel"
title = "Linear"
placement = "split"
command = ["./target/release/herdr-linear"]
# No Windows counterpart: the relative pane command can't be spawned by herdr on
# Windows (see below) — the Windows actions open the pane themselves, by absolute path.

[[actions]]
id = "open-split"
platforms = ["linux", "macos"]
title = "Open Linear panel"
command = ["bash", "scripts/open-split.sh"]

[[actions]]
id = "open-split-windows"
platforms = ["windows"]
title = "Open Linear panel"
command = ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "scripts/open-split-windows.ps1"]

[[actions]]
id = "open-tab"
platforms = ["linux", "macos"]
title = "Open Linear panel (tab)"
command = ["bash", "scripts/open-tab.sh"]

[[actions]]
id = "open-tab-windows"
platforms = ["windows"]
title = "Open Linear panel (tab)"
command = ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "scripts/open-tab-windows.ps1"]
```

## Data flow

### Release (tag push)

1. `git tag vX.Y.Z && git push --tags` triggers `.github/workflows/release.yml`.
2. A verification step aborts the whole run if `X.Y.Z` doesn't equal both
   `Cargo.toml`'s and `herdr-plugin.toml`'s `version` — a release can never publish
   assets that disagree with the manifest a consumer would install.
3. The build matrix (macOS ×2, Linux musl, Windows msvc — `dtolnay/rust-toolchain@stable`,
   `musl-tools` installed for the musl leg) compiles each target, stages the binary as
   `dist/<asset-name>`, and writes its SHA-256 next to it.
4. The publish job downloads all matrix artifacts, concatenates the checksums into a
   single `SHA256SUMS`, writes the release commit to `COMMIT` (informational only — see
   Error handling), and creates/updates the GitHub Release for that tag with all of it
   attached.

### Install (`herdr plugin install`)

1. Herdr clones the repo and runs the platform-matching `[[build]]` command.
2. `fetch-or-build.sh`/`.ps1` reads the plugin's own declared version from
   `Cargo.toml`, detects OS/arch, and — unless any fallback condition below fires —
   downloads `herdr-linear-<triple>[.exe]` and `SHA256SUMS` from the GitHub Release
   tagged `v<version>`, verifies the checksum, and installs the verified binary at
   `target/release/herdr-linear[.exe]` (the exact path the manifest's `[[panes]]`
   entry already expects).
3. On any fallback condition, it prints why and runs
   `cargo build --release --features plugin` instead — identical to today's behavior.

### Runtime action flow — Windows

**Windows pane-spawn limitation:** herdr resolves a manifest `[[panes]] command`'s
relative program path against herdr's own working directory, not the plugin root, when
spawning via `CreateProcessW` — so `plugin pane open --entrypoint linear-panel` fails
with `ERROR_PATH_NOT_FOUND` on Windows (confirmed on real hardware by the
`herdr-file-viewer` author). herdr also stores the plugin root as a `\\?\` verbatim
path. The Windows actions therefore never go through `plugin pane open`; they open the
pane themselves, by absolute path:

`scripts/open-split-windows.ps1` (mirrors `open-file-viewer.ps1`):
1. Force UTF-8 console encoding (PowerShell 5.1's legacy code page can corrupt herdr's
   JSON on non-ASCII pane titles/paths).
2. Resolve the plugin root from `$PSScriptRoot`'s parent, stripping the `\\?\` prefix,
   to get an absolute path to `target\release\herdr-linear.exe`.
3. Compute the OPEN/FOCUS/CLOSE decision exactly as today: `herdr pane list` JSON piped
   into `herdr-linear.exe --launch-decision`.
4. On `OPEN`: fetch the plugin's config dir via `herdr plugin config-dir herdr-linear`
   (strip `\\?\`) and pass it as `--env HERDR_PLUGIN_CONFIG_DIR=<dir>` to
   `herdr pane split --direction right --cwd <focused-pane-cwd> --focus`. herdr only
   auto-injects `HERDR_PLUGIN_CONFIG_DIR` into panes it spawns from the manifest itself
   — since this script spawns the binary manually, it must forward it explicitly, or
   `herdr-linear.exe` silently can't find its `config.toml`/API key on Windows.
5. Run the binary into the new pane via `herdr pane run <id> "& \"<absolute-exe-path>\""`
   — the call operator plus quoting is required so a space in the install path (e.g.
   `C:\Users\Max Mustermann\...`) doesn't split the command and fail the launch.
6. `pane rename <id> Linear` so a later invocation's launch-decision recognizes it.
7. `FOCUS`/`CLOSE` decisions behave exactly as the Unix script (`pane zoom --on/--off`,
   `pane close`).

`scripts/open-tab-windows.ps1` mirrors `open-file-viewer-tab.ps1`: same structure, but
`herdr tab create --cwd <cwd> --label Linear --focus` instead of `pane split`, decision
via `--launch-decision-tab`, and a `SWITCHTAB <id>` branch that falls back to opening a
fresh tab if the target tab vanished between the `pane list` snapshot and execution
(race).

No changes to the Rust binary are needed — `--launch-decision` and
`--launch-decision-tab` already exist and are simply called by a second, Windows-native
caller.

## Error handling

Fallback chain in `fetch-or-build.sh`/`.ps1` (each stage falls through to the next;
the last stage is today's status quo):

1. OS/arch not in the release matrix (e.g. Linux aarch64) → build from source.
2. Version unreadable from `Cargo.toml`, no `curl`/`wget` available, download fails, no
   release exists for the declared version, checksum missing or mismatched → build from
   source. A binary is **never** installed without a verified SHA-256 match.
3. Only if `cargo` is *also* unavailable: hard failure with a message pointing to
   rustup.rs — identical to today, since install has always required a Rust toolchain
   as the ultimate fallback. Install can never get harder than it is now.
4. `COMMIT` in the release is informational only: it lets the fallback path note when a
   local checkout is ahead of the last released tag. It never blocks using the
   prebuilt binary — the install step matches by *declared version*
   (`Cargo.toml`/`herdr-plugin.toml` ↔ release tag), not by exact commit.

Release-side: the version-match CI step (Data flow → Release, step 2) prevents a
release from ever being published whose assets disagree with the manifest a consumer
would install.

## Testing strategy

- **Structural content tests in Rust** (`tests/manifest.rs`, `tests/launcher_content.rs`,
  `tests/fetch_or_build.rs`, `tests/release_workflow.rs`), run as part of the normal
  `cargo test` — no Windows runner required. They assert required substrings/structure
  rather than executing the scripts: `herdr-plugin.toml` declares all three platforms
  and the expected platform-gated entries; `fetch-or-build.sh`/`.ps1` contain the
  checksum-verify-then-fallback logic; `open-split-windows.ps1`/`open-tab-windows.ps1`
  strip `\\?\`, fetch and forward `HERDR_PLUGIN_CONFIG_DIR`, and use the quoted call
  operator; `release.yml` contains the tag/version verification step. This catches
  regressions (e.g. "forgot to forward the config dir") without needing real
  cross-platform execution for every change.
- **CI as smoke test:** `release.yml` succeeding on a tag push is itself proof all 5
  targets compile.
- **Manual verification checklist** (documented in the implementation plan, not
  automatable): install once on real macOS, Linux, and — most importantly — Windows
  hardware. The Windows pane-spawn workaround is exactly the kind of thing that only
  real hardware can confirm (per the reference plugin's own comments).
- **One-time manual fallback check:** deliberately break a checksum or point at a
  missing release and confirm `fetch-or-build.sh`/`.ps1` cleanly falls back to
  `cargo build --release --features plugin` instead of hard-failing.

## Out of scope / open items for the implementation plan

- Exact `dtolnay/rust-toolchain` pin (design assumes `stable`, rolling).
- Whether `tests/*.rs` content assertions are added as one file or split as listed
  above — listed here for scope estimation, final file boundaries are an
  implementation-plan decision.
- README wording for the new Windows section and updated install instructions.
- Confirming `rustls-tls` behaves identically against Linear's GraphQL endpoint (a
  quick manual live-call check during implementation, not expected to surface issues).
