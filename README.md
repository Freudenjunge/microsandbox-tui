# microsandbox-tui

A Docker `sbx`-style terminal dashboard for [microsandbox](https://github.com/superradcompany/microsandbox).

Manage microVM sandboxes visually — create, start, stop, exec, publish ports,
edit network rules, stream logs — without memorizing CLI flags.

## Prerequisites

- None at startup. If no `msb` runtime is detected, the dashboard shows a
  banner — press `U` to install/update microsandbox from inside the TUI.
- KVM (Linux), Apple Silicon (macOS), or WHP (Windows), for actually running
  sandboxes.

Install microsandbox manually (optional alternative to the in-TUI installer):

```sh
curl -fsSL https://install.microsandbox.dev | sh
```

## Install

```sh
cargo install --path .
```

## Usage

```sh
msb-tui                    # open dashboard
msb-tui create             # open create-sandbox form
```

See `docs/DESIGN.md` for the full design document.