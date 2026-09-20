# microsandbox-tui

A Docker `sbx`-style terminal dashboard for [microsandbox](https://github.com/superradcompany/microsandbox).

Manage microVM sandboxes visually — create, start, stop, exec, publish ports,
edit network rules, stream logs — without memorizing CLI flags.

## Prerequisites

- [`msb` CLI](https://docs.microsandbox.dev/cli/overview) installed and on `$PATH`
- KVM (Linux), Apple Silicon (macOS), or WHP (Windows)

Install microsandbox:

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