<!-- CODEGRAPH_START -->
## CodeGraph

In repositories indexed by CodeGraph (a `.codegraph/` directory exists at the repo root), reach for it BEFORE grep/find or reading files when you need to understand or locate code:

- **MCP tool** (when available): `codegraph_explore` answers most code questions in one call — the relevant symbols' verbatim source plus the call paths between them, including dynamic-dispatch hops grep can't follow. Name a file or symbol in the query to read its current line-numbered source. If it's listed but deferred, load it by name via tool search.
- **Shell** (always works): `codegraph explore "<symbol names or question>"` prints the same output.

If there is no `.codegraph/` directory, skip CodeGraph entirely — indexing is the user's decision.
<!-- CODEGRAPH_END -->

# microsandbox-tui — Project Rules

## Project Identity

A Docker `sbx`-style terminal dashboard for [microsandbox](https://github.com/superradcompany/microsandbox). It talks to microsandbox exclusively through the typed `microsandbox` 0.7.2 Rust SDK and renders a live TUI with `ratatui` + `crossterm` under `tokio`.

## Design Reference

Before changing code, read `docs/DESIGN.md`. All implementation work follows the three-phase roadmap there. Do not add Phase 3 features while Phase 2 is open.

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

- **SDK→DTO mapping logic** (`src/models.rs`, `src/backend/sdk.rs`): unit-test against real captured JSON fixtures in `src/fixtures/` (models layer) and against SDK types directly (mapping layer). Every mapped field must have a test.
- **CreateSpec → SDK builder mapping** (`src/backend/sdk.rs`): unit-test that a `CreateSpec` produces the expected `SandboxBuilder` configuration (ports, volumes, env, network profile).
- **UI code** (`src/ui/*`): not unit-tested; verify by compiling and later by manual smoke tests. Keep it thin — heavy logic lives in backend/action modules.
- Do not write code to make a test pass before you have a failing test, except for pure plumbing/wiring code.

## Architecture Rules

- `src/backend/` is the only place that touches the microsandbox SDK. Define a `MsbBackend` trait so we can swap to a fake backend for tests (`FakeBackend` in `src/backend/fake.rs`).
- UI code never invokes microsandbox (SDK or `msb`) directly. It calls helpers in `src/actions.rs` or methods on the backend trait.
- The event loop lives in `src/app.rs`: `tokio::select!` over crossterm input + metric/list pollers.
- `src/models.rs` owns all data structures and serde deserializers; SDK types must not leak past `src/backend/sdk.rs`.
- Never shell out to `msb` from the TUI. The only `msb` interaction in the codebase is runtime detection/install in `src/runtime.rs`, which inspects the binary on disk and never parses its output.

## Backend / SDK Rules

- Pin the SDK to the target runtime: `microsandbox` 0.7.2 crates (`microsandbox`, `microsandbox-types`, `microsandbox-network = "=0.7.2"`, `microsandbox-utils`). The dashboard banner compares the installed `msb` version against the SDK's `PREBUILT_VERSION`.
- If no `msb` runtime is found at startup, do NOT exit. Show the dashboard banner; the `U` key offers install/update via `setup::install_runtime` on a tokio task.
- The SDK defaults to ATTACHED sandboxes. Every create/start/restart path MUST run detached so sandboxes survive the TUI process.
- Ports and network rules cannot be changed on a running sandbox in v0.7.2 (`SandboxModificationBuilder` has no port methods). Any publish/unpublish/network-rule edit must recreate via the backend:
  1. Read current config via `inspect`.
  2. `stop` the sandbox.
  3. `remove` it (0.7.2 has NO create-with-replace; a name collision errors).
  4. `create` it again including the change.
  5. `start` it.
  6. Warn the user that the rootfs resets unless they snapshot first.
- Default new-sandbox network profile is `public`.
- New sandboxes default to named/persistent (ephemeral is an advanced toggle).

## UI / UX Rules

- Use a responsive dashboard layout (1–3 columns) and concise keybinding hints at the bottom of every screen.
- All destructive actions (remove sandbox, remove volume, recreate for port change) require a confirmation dialog.
- Long-running SDK operations must not block the UI; spawn them on `tokio` tasks and show a spinner/status message.
- Prefer names and IDs exactly as returned by the SDK over guessing or string-parsing.

## Code Style

- Rust 2024 edition idioms.
- Public types/methods have doc comments; private helpers get `//` comments when non-obvious.
- Use `anyhow` for binary-level errors, `thiserror` for library/backend errors. Keep UI error messages user-friendly.

## Commit Rules

- Commit after each completed task (not after every file).
- Use Conventional Commits: `feat(scope): description`, `fix(scope): description`, `refactor(scope):`, `test(scope):`, `docs(scope):`, `chore(scope):`.
- Every commit message body must mention the plan task number it closes (e.g. `Closes phase2#5`).

## File / Plan Tracking

- Active tasks live in `docs/PLAN.md`. Mark completed tasks with `[x]` and update the file in the same commit that completes the work.
- Do not create new files outside the design doc's structure without a good reason and a note in the commit.
