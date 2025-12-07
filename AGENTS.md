# Repository Guidelines

## Project Structure & Module Organization
- `src/main.rs` wires CLI parsing to command handlers.
- `src/cli.rs` defines subcommands and flags via `clap`.
- `src/commands.rs` implements CLI actions (init/list/add/move/edit/tui) and orchestrates storage/UI.
- `src/model.rs` holds core types (`Board`, `Column`, `Note`) and move/update logic.
- `src/storage.rs` loads/saves YAML boards (project `.postit/board.yml` or global data dir).
- `src/ui.rs` contains the `ratatui`/`crossterm` TUI loop.
- Add integration tests under `tests/` or module tests alongside code.

## Build, Test, and Development Commands
- `cargo build` — compile the binary.
- `cargo run -- <subcommand>` — run CLI (e.g., `cargo run -- init`, `cargo run -- list`, `cargo run -- tui`).
- `cargo fmt` — format code with rustfmt.
- `cargo clippy --all-targets --all-features` — lint for common pitfalls.
- `cargo test` — run the test suite (add tests as you contribute).

## Coding Style & Naming Conventions
- Rust 2021 edition; default rustfmt settings (spaces, 100-column width).
- Prefer explicit, small functions; avoid needless clones where possible.
- IDs are lowercase alphanumeric strings (e.g., `todo`, `doing`, `abc123`).
- Keep comments brief and purposeful; favor clear naming over heavy commentary.
- YAML persistence via `serde_yaml`; keep schema stable and human-editable.

## Testing Guidelines
- Use `cargo test` locally before opening PRs.
- Co-locate unit tests with modules for logic; add integration tests for CLI flows in `tests/`.
- Favor deterministic IDs/mocks in tests to avoid flakiness.
- When adding features, cover happy-path and error-path behaviors (missing columns, WIP limits, bad input).

## Commit & Pull Request Guidelines
- Write descriptive commit messages (`verb target`, e.g., `add tui navigation keys`, `fix wip limit check`).
- PRs should include: summary of changes, key commands run (`cargo fmt`, `cargo clippy`, `cargo test`), and any screenshots/GIFs for TUI changes when helpful.
- Link related issues and call out breaking changes or data migrations.

## Architecture & Data Notes
- Board selection prefers nearest `.postit/board.yml`; otherwise uses the global data path from `directories`.
- Data is plain YAML for easy hand-editing and version control; avoid lossy migrations.
- TUI is keyboard-driven (vim-style navigation); keep interactions fast and non-blocking.
