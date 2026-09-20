<!-- CODEGRAPH_START -->
## CodeGraph

In repositories indexed by CodeGraph (a `.codegraph/` directory exists at the repo root), reach for it BEFORE grep/find or reading files when you need to understand or locate code:

- **MCP tool** (when available): `codegraph_explore` answers most code questions in one call — the relevant symbols' verbatim source plus the call paths between them, including dynamic-dispatch hops grep can't follow. Name a file or symbol in the query to read its current line-numbered source. If it's listed but deferred, load it by name via tool search.
- **Shell** (always works): `codegraph explore "<symbol names or question>"` prints the same output.

If there is no `.codegraph/` directory, skip CodeGraph entirely — indexing is the user's decision.
<!-- CODEGRAPH_END -->

# microsandbox-tui — Project Rules

## Project Identity

A Docker `sbx`-style terminal dashboard for [microsandbox](https://github.com/superradcompany/microsandbox). It drives the installed `msb` CLI via `--format json` and renders a live TUI with `ratatui` + `crossterm` under `tokio`.

## Design Reference

Before changing code, read `docs/DESIGN.md`. All implementation work follows the three-phase roadmap there. Do not add Phase 2/3 features while Phase 1 is open.

## Mandatory Verification Commands

Run these in order before every commit or after any non-trivial change:

```sh
cargo fmt --check
cargo clippy -- -D warnings
cargo test
cargo check
```

Fix all warnings and test failures. Treat `clippy` lints as errors in this project.

## TDD / Testing Policy

- **Logic that parses CLI JSON** (`src/models.rs`, `src/backend/cli.rs`): unit-test with real captured JSON fixtures in `src/fixtures/`. Every parsed field must have a test.
- **CLI command building** (`src/actions.rs` or equivalent): unit-test the generated `msb` argument vectors.
- **UI code** (`src/ui/*`): not unit-tested; verify by compiling and later by manual smoke tests. Keep it thin — heavy logic lives in backend/action modules.
- Do not write code to make a test pass before you have a failing test, except for pure plumbing/wiring code.

## Architecture Rules

- `src/backend/` is the only place that calls `msb` via `tokio::process::Command`. Define a `MsbBackend` trait so we can swap to a fake backend for tests.
- UI code never invokes `msb` directly. It calls helpers in `src/actions.rs` or methods on the backend trait.
- The event loop lives in `src/app.rs`: `tokio::select!` over crossterm input + metric/list pollers.
- `src/models.rs` owns all data structures and serde deserializers.
- Every `msb` listing/inspection call must use `--format json` (or `--json` for logs). Never parse human-readable output.

## Backend / msb Rules

- Target the installed CLI version: `msb 0.7.2` at `~/.local/bin/msb`. If the TUI binary cannot find `msb` on startup, print the install command and exit cleanly.
- Ports and network rules cannot be changed on a running sandbox in v0.7.2. Any publish/unpublish/network-rule edit must:
  1. Read current config via `msb inspect <name> --format json`.
  2. Stop the sandbox with `msb stop <name>`.
  3. Remove it with `msb rm <name>` (0.7.2 has NO `create --replace` flag; a name collision errors).
  4. Recreate it with `msb create --name <name> ... <image>` including the change.
  5. Start it with `msb start <name>`.
  6. Warn the user that the rootfs resets unless they snapshot first.
- Default new-sandbox network profile is `public` (`--net public`).
- New sandboxes default to named/persistent (ephemeral is an advanced toggle).

## UI / UX Rules

- Use a responsive dashboard layout (1–3 columns) and concise keybinding hints at the bottom of every screen.
- All destructive actions (remove sandbox, remove volume, recreate for port change) require a confirmation dialog.
- Long-running `msb` operations must not block the UI; spawn them on `tokio` tasks and show a spinner/status message.
- Prefer `msb` IDs and names from JSON over guessing or string-parsing.

## Code Style

- Rust 2024 edition idioms.
- Public types/methods have doc comments; private helpers get `//` comments when non-obvious.
- Use `anyhow` for binary-level errors, `thiserror` for library/backend errors. Keep UI error messages user-friendly.

## Commit Rules

- Commit after each completed task (not after every file).
- Use Conventional Commits: `feat(scope): description`, `fix(scope): description`, `refactor(scope):`, `test(scope):`, `docs(scope):`, `chore(scope):`.
- Every commit message body must mention the Phase-1 task number it closes (e.g. `Closes phase1#3`).

## File / Plan Tracking

- Active tasks live in `docs/PLAN.md`. Mark completed tasks with `[x]` and update the file in the same commit that completes the work.
- Do not create new files outside the design doc's structure without a good reason and a note in the commit.
