# Contributing to CopyDeck

Thank you for your interest in contributing!  This document covers how to set
up a development environment, run tests, and submit changes.

## Prerequisites

| Tool | Version | Install |
|------|---------|---------|
| Rust | ≥ 1.75 | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| System libs | — | `sudo bash packaging/install-system-deps.sh` |

## Development setup

```bash
git clone https://github.com/your-org/copydeck
cd copydeck

# Build in debug mode
cargo build

# Run all tests
cargo test

# Check formatting and lints before committing
cargo fmt --check
cargo clippy --all-targets
```

## Running the binary locally

```bash
cargo run -- check-deps
cargo run -- config
```

## Project layout

```
src/
  lib.rs          public library (config, storage, utils)
  main.rs         binary entry point + CLI dispatch
  cli.rs          clap argument definitions
  config.rs       user configuration (serde/toml)
  storage.rs      SQLite history + pinned items
  utils/
    display.rs    X11 / Wayland detection
    deps.rs       system dependency checker
tests/
  storage_tests.rs  integration tests (in-memory SQLite)
packaging/
  copydeck.service  systemd user unit file
  install-system-deps.sh
```

## Coding standards

- **Formatting**: `cargo fmt` (enforced by CI).
- **Lints**: `cargo clippy -- -D warnings` must pass with no warnings.
- **Tests**: every public function in `storage.rs` and `config.rs` has at
  least one test.  New behaviour requires new tests.
- **Docs**: public items carry `///` doc comments.  Non-obvious logic has
  inline `//` comments.
- **Errors**: use `anyhow` with `.context("…")` for actionable messages.
  Avoid `unwrap()` outside of tests and infallible contexts.

## Submitting a pull request

1. Fork the repository and create a feature branch from `main`.
2. Make your changes, ensuring `cargo test`, `cargo fmt --check`, and
   `cargo clippy` all pass.
3. Write or update tests for any changed behaviour.
4. Open a pull request against `main` with a clear description of what
   changed and why.

## Reporting issues

Please open a GitHub issue with:
- Your Ubuntu version (`lsb_release -a`)
- Your display server (`echo $WAYLAND_DISPLAY $DISPLAY`)
- Output of `copydeck check-deps`
- Steps to reproduce
