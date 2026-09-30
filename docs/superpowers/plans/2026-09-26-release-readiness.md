# Release Readiness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `microsandbox-tui` publishable as a public GitHub repo: correct `msb-tui` binary, filled LICENSE, AGENTS.md removed, full public README, CI workflow, tag-triggered release workflow with binaries, and repo metadata polish.

**Architecture:** Docs-and-config only — no product-code restructuring. One Cargo metadata fix (`[[bin]]`), one display-string rephrase, one LICENSE edit, one file deletion, one full README rewrite, two GitHub Actions workflows, and PLAN.md bookkeeping. All code tests already pass; the only code-adjacent changes are the `[[bin]]` table (no behavior change) and template description strings (display-only; no test pins them — verified 2026-09-26 via grep for "pulls"/"description" asserts in `src/template.rs` and `src/ui/template_picker.rs`).

**Tech Stack:** Rust 2024 edition, GitHub Actions (workflow YAML), git, `gh` CLI.

**Spec:** In-chat design approved 2026-09-26 in session `ses_f207c8da3ffeqs80YFbTx5DGsl` (brainstorming bounded path). Verbatim decisions:
- Publish target: **GitHub only** (no crates.io).
- README: **Full public README** (hero, badges, why, features, screenshots section, requirements, install, usage, keybindings, templates, how-it-works, development, license).
- Screenshots: **user provides later** — README has a ready section with clearly-marked slots.
- Actions: **CI = the project's 4 gates**; **Release = tag-triggered, Linux+macOS binaries**.
- Binary name: **`msb-tui`** via `[[bin]]` (package stays `microsandbox-tui`).
- Release version: **keep 0.1.0**.
- AGENTS.md: **delete outright** (no CONTRIBUTING.md).
- Docker sbx references: **keep + trademark disclaimer** in README; rephrase "official sbx hub" → "sbx org on Docker Hub" in template descriptions.
- LICENSE appendix placeholder → **`Copyright 2026 Freudenjunge`**.
- Repo metadata: description + topics via `gh`.
- Publish sequence (controller-executed at the end): land commits → push main → CI green → tag `v0.1.0` → release workflow → user flips visibility.

## Global Constraints

- Mandatory gates before every commit, in order: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `cargo check`. All must pass.
- Conventional Commits: `feat(scope):`, `fix(scope):`, `refactor(scope):`, `test(scope):`, `docs(scope):`, `chore(scope):`. Commit body mentions the plan task it closes (e.g. `Closes release#3`).
- PLAN.md: mark completed tasks `[x]` in the same commit that completes the work. New "Phase 3 — Release readiness" section records the tasks.
- Version stays 0.1.0. Package name stays `microsandbox-tui`. Installed command becomes `msb-tui`.
- README must contain: trademark disclaimer (Docker, Superrad Company/microsandbox, and the products shipped as templates), screenshot slots, per-platform install for Linux x86_64/ARM64 + macOS ARM64, keybindings sourced from the actual handlers (not the slightly-stale help overlay — see Task 5), template TOML example, four-gates development section, Apache-2.0 license section.
- Workflow YAML must be valid (actionlint, or a YAML parse check if actionlint is unavailable) and use well-known actions (`actions/checkout`, `dtolnay/rust-toolchain`, `Swatinem/rust-cache`, `actions/upload-artifact`, `actions/download-artifact`, `softprops/action-gh-release`) and the native ARM runners (`ubuntu-24.04-arm`, `macos-latest`).
- Delete AGENTS.md with `git rm`; no CONTRIBUTING.md replacement.
- The `[[bin]]` addition must not change clap's `#[command(name = "msb-tui")]` line in `src/main.rs` (no code change at all).
- Template description changes are display-only: the full test suite is the ground truth; no test pins description text.
- `examples/` is an empty untracked directory — leave it alone.

## Review Focus

Most likely failure modes for this docs/config work, each with its pinning check in the owning task:

1. **Workflow YAML that doesn't parse or references wrong runners/actions** — Tasks 3/4 validate with actionlint (installed via `bash <(curl ...)` if missing — see task) or a YAML parse; controller re-checks with `gh workflow list` after push (Task 6).
2. **README commands/asset names that don't match reality** — Task 5 verification greps the README for the real flag (`--create`), the real binary (`msb-tui`), and asset names matching the release workflow's `msb-tui-<tag>-<target>.tar.gz` pattern exactly.
3. **Dead image links in the screenshots section** — Task 5 uses explicit TODO-marked text slots, not image links; reviewer checks no `![](` points at missing files.
4. **Test pins on template descriptions breaking on rephrase** — Task 1 runs the full four-gate suite as ground truth.
5. **Stray files landing in commits** — every task ends with `git status --porcelain` reviewed against the task's file list before committing; the task reviewer checks `git show --stat` per commit.

---

### Task 1: Cargo `[[bin]]` fix + template wording

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/templates/sbx/opencode.toml` (only file with the phrase; verified via grep)
- Modify: `docs/PLAN.md` (append Phase 3 section; mark item 1)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `cargo build` / `cargo install` produce a binary named `msb-tui`; the OpenCode template description no longer says "official sbx hub".

- [ ] **Step 1: Add the `[[bin]]` table to Cargo.toml**

Insert between the `[package]` section and `[dependencies]`:

```toml
[[bin]]
name = "msb-tui"
path = "src/main.rs"
```

- [ ] **Step 2: Verify the binary name**

Run: `cargo build 2>&1 | tail -1 && ls target/debug/msb-tui && (ls target/debug/microsandbox-tui 2>/dev/null || echo "old name gone")`
Expected: build succeeds, `target/debug/msb-tui` exists, old name absent.

- [ ] **Step 3: Rephrase the template description**

In `src/templates/sbx/opencode.toml`, change the `meta.description` line from:

```toml
description = "OpenCode, the open-source terminal coding agent, from the official sbx hub — pulls 0.4k"
```

to:

```toml
description = "OpenCode, the open-source terminal coding agent, from the sbx org on Docker Hub — pulls 0.4k"
```

Run first to confirm only this file matches: `grep -l "official sbx hub" src/templates/sbx/*.toml src/templates/*.toml`
Expected: only `src/templates/sbx/opencode.toml`.

- [ ] **Step 4: Append the Phase 3 section to docs/PLAN.md**

Append exactly:

```markdown
---

# Phase 3 — Release readiness (2026-09-26)

Goal: make the repo publishable as a public GitHub repo — GitHub-only for
now (no crates.io). Design approved in-session 2026-09-26: [[bin]] msb-tui,
LICENSE appendix fix, AGENTS.md removal, full public README, CI workflow
(4 gates), tag-triggered release workflow (Linux x86_64/ARM64 + macOS ARM64
binaries), repo metadata, tag v0.1.0.

Plan: `docs/superpowers/plans/2026-09-26-release-readiness.md`

## Task List

### 1. Cargo [[bin]] msb-tui + template wording
- [x] `[[bin]] name = "msb-tui"` — installed command matches all docs.
- [x] Template description "official sbx hub" → "sbx org on Docker Hub".

### 2. LICENSE appendix + AGENTS.md removal
- [ ] Appendix `Copyright [yyyy] [name]` → `Copyright 2026 Freudenjunge`.
- [ ] AGENTS.md deleted (rules live in DESIGN.md/PLAN.md; user decision).

### 3. README rewrite (public storefront)
- [ ] Full public README: hero+badges, why, features, screenshots (slots),
      requirements matrix, install (3 ways), usage, keybindings, templates,
      how-it-works, development, license+disclaimer.

### 4. CI workflow
- [ ] `.github/workflows/ci.yml`: push/PR to main; fmt+clippy+test+check.

### 5. Release workflow
- [ ] `.github/workflows/release.yml`: tag v*; gates; build matrix
      (x86_64-linux-gnu, aarch64-linux-gnu, aarch64-apple-darwin);
      tar.gz assets; GitHub Release.
```

(Item 1 marked `[x]` — this commit completes it.)

- [ ] **Step 5: Run the four gates**

Run: `cargo fmt --check && cargo clippy -- -D warnings && cargo test && cargo check`
Expected: all four pass (276 tests, 1 ignored).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml src/templates/sbx/opencode.toml docs/PLAN.md
git commit -m "chore(meta): install as msb-tui; neutral sbx-hub wording in one template

[[bin]] name = \"msb-tui\" makes cargo install produce the command every
doc promises; the package name stays microsandbox-tui. The OpenCode
template description drops 'official sbx hub' for 'sbx org on Docker
Hub' (trademark hygiene before going public). PLAN.md gains the Phase 3
release-readiness section; its item 1 is done in this commit.

Closes release#1"
```

---

### Task 2: LICENSE appendix + AGENTS.md removal

**Files:**
- Modify: `LICENSE`
- Delete: `AGENTS.md`
- Modify: `docs/PLAN.md` (mark item 2)

**Interfaces:**
- Consumes: Phase 3 section exists in PLAN.md (Task 1).
- Produces: LICENSE appendix reads `Copyright 2026 Freudenjunge`; AGENTS.md absent from the branch.

- [ ] **Step 1: Fix the LICENSE appendix copyright line**

In `LICENSE`, find:

```
   Copyright [yyyy] [name of copyright owner]
```

Replace with:

```
   Copyright 2026 Freudenjunge
```

(Only the appendix placeholder changes. The top notice `Copyright (c) 2026 Freudenjunge` is already correct — do not touch it.)

- [ ] **Step 2: Delete AGENTS.md**

Run: `git rm AGENTS.md`

- [ ] **Step 3: Mark PLAN.md item 2 `[x]`**

In `docs/PLAN.md` Phase 3 section, change the two item-2 checkboxes to `[x]`.

- [ ] **Step 4: Run the four gates**

Run: `cargo fmt --check && cargo clippy -- -D warnings && cargo test && cargo check`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add LICENSE AGENTS.md docs/PLAN.md
git commit -m "chore(repo): fill LICENSE appendix; remove AGENTS.md

The Apache-2.0 appendix still carried the template placeholder; it now
names the copyright holder (matching the top notice). AGENTS.md (agent
workflow rules) is removed for the public repo per maintainer decision —
DESIGN.md and PLAN.md carry the durable project rules. PLAN.md Phase 3
item 2 marked done.

Closes release#2"
```

---

### Task 3: CI workflow

**Files:**
- Create: `.github/workflows/ci.yml`
- Modify: `docs/PLAN.md` (mark item 4)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `.github/workflows/ci.yml` — CI on push/PR to main running the 4 gates. Takes effect when Task 6 pushes it to GitHub.

- [ ] **Step 1: Write `.github/workflows/ci.yml`**

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

env:
  CARGO_TERM_COLOR: always

jobs:
  check:
    name: fmt + clippy + test + check
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Format
        run: cargo fmt --check
      - name: Clippy
        run: cargo clippy -- -D warnings
      - name: Test
        run: cargo test
      - name: Check
        run: cargo check
```

- [ ] **Step 2: Validate the workflow YAML**

Try, in order:
1. `actionlint .github/workflows/ci.yml` (if installed)
2. If not: `sudo bash -c "curl -sSL https://raw.githubusercontent.com/rhysd/actionlint/main/scripts/download-actionlint.bash | bash" && ./actionlint .github/workflows/ci.yml`
3. If offline/no curl: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))" && echo YAML-OK`

Expected: no errors.

- [ ] **Step 3: Mark PLAN.md item 4 `[x]`**

In `docs/PLAN.md` Phase 3 section, change the ci.yml checkbox to `[x]`.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml docs/PLAN.md
git commit -m "ci: four-gates workflow on push/PR to main

Runs the project's mandatory verification gates exactly: cargo fmt
--check, clippy -D warnings, test, check — with rust-cache for speed.
Re-creates CI (removed in Phase 2 at maintainer request; re-adding is
the Phase 3 decision, recorded in PLAN.md).

Closes release#3"
```

---

### Task 4: Release workflow

**Files:**
- Create: `.github/workflows/release.yml`
- Modify: `docs/PLAN.md` (mark item 5)

**Interfaces:**
- Consumes: `[[bin]] name = "msb-tui"` from Task 1 — the built binary path is `target/<target>/release/msb-tui`.
- Produces: `.github/workflows/release.yml` — on `v*` tags: gates, native-runner build matrix, `msb-tui-<tag>-<target>.tar.gz` + `.sha256` assets, GitHub Release. README (Task 5) must reference asset names in exactly this pattern: `msb-tui-v0.1.0-x86_64-unknown-linux-gnu.tar.gz` etc.

- [ ] **Step 1: Write `.github/workflows/release.yml`**

```yaml
name: Release

on:
  push:
    tags:
      - "v*"

env:
  CARGO_TERM_COLOR: always

jobs:
  gates:
    name: Gates (fmt + clippy + test + check)
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Format
        run: cargo fmt --check
      - name: Clippy
        run: cargo clippy -- -D warnings
      - name: Test
        run: cargo test
      - name: Check
        run: cargo check

  build:
    name: Build ${{ matrix.target }}
    needs: [gates]
    runs-on: ${{ matrix.os }}
    timeout-minutes: 60
    strategy:
      fail-fast: false
      matrix:
        include:
          - target: x86_64-unknown-linux-gnu
            os: ubuntu-latest
          - target: aarch64-unknown-linux-gnu
            os: ubuntu-24.04-arm
          - target: aarch64-apple-darwin
            os: macos-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - uses: Swatinem/rust-cache@v2
        with:
          key: ${{ matrix.target }}
      - name: Build release binary
        run: cargo build --release --target ${{ matrix.target }}
      - name: Pack archive
        run: |
          STAGING="msb-tui-${{ github.ref_name }}-${{ matrix.target }}"
          mkdir -p "$STAGING"
          cp "target/${{ matrix.target }}/release/msb-tui" "$STAGING/"
          cp README.md LICENSE "$STAGING/"
          tar -czf "$STAGING.tar.gz" "$STAGING"
          sha256sum "$STAGING.tar.gz" > "$STAGING.tar.gz.sha256"
      - name: Upload artifact
        uses: actions/upload-artifact@v4
        with:
          name: msb-tui-${{ github.ref_name }}-${{ matrix.target }}
          path: |
            msb-tui-${{ github.ref_name }}-${{ matrix.target }}.tar.gz
            msb-tui-${{ github.ref_name }}-${{ matrix.target }}.tar.gz.sha256

  publish:
    name: Create GitHub Release
    needs: [build]
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - name: Download artifacts
        uses: actions/download-artifact@v4
        with:
          path: dist
          merge-multiple: true
      - name: Create Release with assets
        uses: softprops/action-gh-release@v2
        with:
          generate_release_notes: true
          files: |
            dist/*.tar.gz
            dist/*.tar.gz.sha256
```

- [ ] **Step 2: Verify these invariants — they must ALL hold in the final file:**
1. Gates job has `actions/checkout@v4` and `dtolnay/rust-toolchain@stable` steps.
2. Build job has `needs: [gates]`; publish has `needs: [build]`.
3. Exactly one `Upload artifact` step in the build job; asset names all use `msb-tui-${{ github.ref_name }}-${{ matrix.target }}`.
4. Targets: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`.
5. No `VERSION` env var is defined or used — `${{ github.ref_name }}` is used directly (it already contains the `v` prefix).

- [ ] **Step 3: Validate the workflow YAML**

Same ladder as Task 3 (`actionlint`, then download-script install, then python fallback). Expected: no errors.

- [ ] **Step 4: Mark PLAN.md item 5 `[x]`**

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/release.yml docs/PLAN.md
git commit -m "ci: tag-triggered release workflow with Linux/macOS binaries

On v* tags: run the four gates, then build a native-runner matrix —
x86_64-unknown-linux-gnu (ubuntu-latest), aarch64-unknown-linux-gnu
(ubuntu-24.04-arm), aarch64-apple-darwin (macos-latest). Each target
packs msb-tui-<tag>-<target>.tar.gz + sha256 and uploads it as an
artifact; the publish job downloads all three and creates the GitHub
Release (generated notes) via softprops/action-gh-release@v2.

Closes release#4"
```

---

### Task 5: README rewrite

**Files:**
- Rewrite: `README.md`
- Modify: `docs/PLAN.md` (mark item 3)

**Interfaces:**
- Consumes: `msb-tui` binary name (Task 1), asset pattern `msb-tui-<tag>-<target>.tar.gz` (Task 4), keybinding facts listed below, template TOML format from `src/template.rs`.
- Produces: the public README.

**Verified keybinding facts (from `src/app.rs` / `src/ui/logs.rs` handlers, 2026-09-26 — the help overlay has two stale rows; the README uses THESE):**
- Global: `q` quit, `?` help overlay.
- Dashboard: `↑/↓` select card; `1/2/3` detail tabs (overview/logs/ports); `Tab` cycle tabs; `c` create; `x` exec — interactive shell in a new terminal window; `s` start/stop toggle (state-dependent); `r` restart; `Del` remove (confirm); `U` install/update microsandbox runtime.
- Logs tab: `f` toggle follow; `g` grep filter (enters grep input mode; `/` is NOT a separate binding); `s` source filter (stdout/stderr/system); `↑↓`/`PgUp`/`PgDn` scroll.
- Ports tab: `+` or `p` publish a port (form → confirm; recreates via snapshot/restore); `-` or `_` unpublish selected binding (confirm); `↑/↓` select binding; `Esc`/`q`/`1` back to overview.
- Create flow (`View::Create`): `↑/↓` navigate (row 0 = "Create from scratch"); `Enter` create from the selected template directly; `e` open the form prefilled from the template; `n` open the empty form; `d` delete a user template (confirm); `Ctrl+S` save the current form as a template; `Esc` back.
- First-run: no `msb` runtime needed at startup — banner offers `U` = install/update via `setup::install_runtime` on a tokio task.

- [ ] **Step 1: Write the full README**

Write polished prose (no placeholders except the screenshot slots). Structure, top to bottom:

1. **Centered header** — `<div align="center">` … `</div>` containing: `<h1>msb-tui</h1>`, the one-liner ("A Docker `sbx`-style terminal dashboard for [microsandbox](https://github.com/superradcompany/microsandbox)."), badges: CI (`[![CI](https://github.com/Freudenjunge/microsandbox-tui/actions/workflows/ci.yml/badge.svg)](https://github.com/Freudenjunge/microsandbox-tui/actions/workflows/ci.yml)`), Release (`[![Release](https://img.shields.io/github/v/release/Freudenjunge/microsandbox-tui)](https://github.com/Freudenjunge/microsandbox-tui/releases/latest)`), License (`[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)`).
2. **Why** — 2-3 sentences: manage microVM sandboxes visually — create from templates, start/stop/restart, exec a shell, stream logs, publish ports — without memorizing CLI flags; talks to microsandbox through the typed Rust SDK, never scraping CLI output.
3. **Features** — bullets: live sandbox rail with cards (status, image, CPU gauge, memory, net I/O rates, uptime, mounts); Overview/Logs/Ports detail tabs per sandbox; 20 built-in templates (Shell + the 19-harness sbx Hub catalog); user-defined TOML templates with built-in shadowing; snapshot-preserving port publish/unpublish (data survives the recreate); in-TUI microsandbox runtime install/update; polished dark theme, responsive from small to wide terminals.
4. **Screenshots** — section ready for the maintainer's captures, NO image links yet:

   ```markdown
   ## Screenshots

   > 📸 Captures coming soon — the TUI in action:
   >
   > | Capture | What it shows |
   > |---|---|
   > | dashboard | sandbox rail + detail tabs |
   > | create | template picker |
   > | ports | ports tab |
   >
   > PRs adding captures under `docs/img/` are welcome.
   ```

5. **Requirements** — table: Linux x86_64/ARM64 with KVM; macOS Apple Silicon (Hypervisor.framework — Intel Macs unsupported, a microsandbox limitation); Windows 11 x64/ARM64 with WHP (preview, per microsandbox docs). Plus a line: the `msb` runtime is NOT required at startup — a banner offers in-TUI install (`U`).
6. **Installation** — three subsections:
   - **Prebuilt binary (Linux/macOS)**: link to Releases; example for `msb-tui-v0.1.0-x86_64-unknown-linux-gnu.tar.gz` plus a table of the three target triplets (`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`); sha256 verification example.
   - **From crates.io-adjacent**: `cargo install --git https://github.com/Freudenjunge/microsandbox-tui.git` (installs `msb-tui`).
   - **From source**: `git clone` + `cargo install --path .`.
7. **Usage** — `msb-tui` opens the dashboard; `msb-tui --create` opens the create form directly (the clap flag is `--create`, defined `#[arg(short, long)]` on `src/main.rs:58-59`); first-run story (banner + `U`); short flow walk-through.
8. **Keybindings** — tables from the verified facts above (Global/Dashboard/Logs/Ports/Create).
9. **Templates** — built-in catalog table (20 rows: name + image + one-line description derived from the TOML `meta.description` values, translated to English where German — `shell` is currently "Nackte Sandbox mit CWD-Mount — generischer Einstieg" → render it in English as "Bare sandbox with CWD mount — generic starting point" in the README table only, no code change); shadowing rule (`~/.config/microsandbox-tui/templates/<id>.toml` shadows built-ins by id); clean example TOML:

   ```toml
   [meta]
   name = "My OpenCode box"
   description = "OpenCode with a bigger brain"

   [spec]
   image = "sbx/opencode-image"
   name = "my-opencode"
   cpus = 4
   memory = "8G"
   mount_cwd = true
   ports = ["127.0.0.1:4096:4096"]
   ```

   Then a note on `net_profile` values and `mount_cwd = true` as the sbx convention.
10. **How it works** — 3-4 sentences: typed `microsandbox` 0.7.2 SDK; sandboxes run detached so they survive the TUI process; port changes snapshot → stop → remove → restore with new ports → reapply config, so data survives; link `docs/DESIGN.md`.
11. **Development** — the four gates verbatim, `cargo test` count (276 tests), link `docs/PLAN.md`.
12. **License** — Apache-2.0, link LICENSE + this paragraph:

    "microsandbox-tui is an independent community project. It is not affiliated with, endorsed by, or sponsored by Superrad Company (microsandbox), Docker (the `sbx` TUI and the sbx Hub org), or the maintainers of the products shipped as templates (Claude Code, OpenCode, Aider, Cursor, Codex, and others). Product names and trademarks belong to their respective owners and are used here only to describe what this tool manages."

13. **Acknowledgements** — one line: inspired by Docker's `sbx` TUI; built on [microsandbox](https://github.com/superradcompany/microsandbox).

- [ ] **Step 2: Self-check the README**

Run all; expected results:
- `grep -n "create" src/main.rs | grep arg` → shows `#[arg(short, long)] create: bool` (confirms `--create`).
- `grep -nE "127\.0\.7\.1|githui|msb-tbui" README.md` → no matches.
- `grep -c "![" README.md` → 0 (no image links yet).
- Every install command uses `msb-tui`; every asset name matches `msb-tui-v0.1.0-<target>.tar.gz` with the three real triplets.
- Badge URLs point at `Freudenjunge/microsandbox-tui`.

- [ ] **Step 3: Mark PLAN.md item 3 `[x]`**

- [ ] **Step 4: Gates + commit**

Run: `cargo fmt --check && cargo clippy -- -D warnings && cargo test && cargo check`
Then:

```bash
git add README.md docs/PLAN.md
git commit -m "docs(readme): full public README for the public launch

Hero + badges, why, features, screenshot slots (maintainer provides),
requirements matrix, three install paths, usage, keybindings from the
real key handlers, template catalog + example, how-it-works,
development gates, Apache-2.0 + trademark disclaimer. PLAN.md Phase 3
item 3 marked done.

Closes release#5"
```

---

### Task 6: Final whole-branch review + publish (controller-executed; no subagent)

**Files:**
- No new files.

**Interfaces:**
- Consumes: all prior tasks' commits on `release-readiness`.
- Produces: merged main, pushed origin, tag `v0.1.0`, repo metadata, GitHub Release with 6 assets, user flips visibility.

- [ ] **Step 1: Run the full gate suite on the branch** — all four, green.
- [ ] **Step 2: Final whole-branch review** — dispatch the final reviewer (most capable model) with the review package (MERGE_BASE = `ee8856a`).
- [ ] **Step 3: One fix dispatch if findings + one scoped re-review; adjudicate residuals.**
- [ ] **Step 4: finishing-a-development-branch** — present merge/push options to the maintainer.
- [ ] **Step 5: Merge to main, push main; watch CI go green** (`gh run watch` / `gh run list`).
- [ ] **Step 6: Set repo metadata via `gh`** — description: "A Docker sbx-style terminal dashboard for microsandbox — manage microVM sandboxes visually."; topics: `tui ratatui microsandbox microvm sandbox rust terminal`; homepage unset.
- [ ] **Step 7: STOP and ask the maintainer before tagging** — the tag publishes the public GitHub Release (publish = stop class). On approval: annotated tag `v0.1.0` on the merged main commit, push tag.
- [ ] **Step 8: Watch the release workflow; verify** `gh release view v0.1.0` shows 6 assets (3 tar.gz + 3 sha256).
- [ ] **Step 9: The user flips visibility to public** (their action or one `gh repo edit --visibility public` on their word).
- [ ] **Step 10: Cleanup** — remove the worktree, delete the branch, delete the SDD workspace.

## Notes for executors

- `docs/PLAN.md` item → plan-task mapping (both directions): item 1 → Task 1, item 2 → Task 2, item 4 → Task 3, item 5 → Task 4, item 3 → Task 5. Commit messages close plan-task numbers (`release#N` = Task N).
- The help overlay (`src/ui/help.rs`) has two stale rows (`g / /` grep and ports `r` refresh); the README documents the real handlers per the verified-facts block in Task 5. Do not "fix" help.rs in this plan — out of scope.
- Worktree: `.worktrees/release-readiness`, branch `release-readiness`, base commit `ee8856a` (the .gitignore commit). All file paths in tasks are relative to the worktree root.
- Task 4 Step 1's YAML block intentionally documents its known defect inline and fixes it in Step 2 — read both before writing the file.
