# CLAWIE.md

This file provides development guidance and command references for Clawie (and any other agentic AI coding assistants) when working in the ShrimpAI Clawie workspace.

## Technology Stack & Verification

- **Rust Crate / Runtime**: Located under `rust/` (the core workspace).
  - Format code: `cargo fmt`
  - Lint check: `cargo clippy --workspace --all-targets -- -D warnings`
  - Run all tests: `cargo test --workspace`
- **Python Mirror / Codebase**:
  - Run Sync Auditor CLI from root: `./scripts/check_rust_python_sync.py`
  - Run Python unit tests: `python3 -m unittest tests/test_rust_python_sync.py`

## Repository Structure & Parity

- `rust/` contains the Rust crates for the active CLI, runtime implementation, and MCP servers.
- `src/` contains Python source code modules that must stay in perfect sync between `rust-clawie/src` and `python-clawie/src`.
- `tests/` contains Python validation surfaces.
- **Critical Sync Agreement**:
  - Any Python code changes or additions made to `rust-clawie/src` **must** be synchronized directly into `python-clawie/src`.
  - The script `./scripts/check_rust_python_sync.py` must run and exit with `0` successfully in CI/pre-commit check.

## Lean Ladder (Lazy Senior Dev Mode)

Always enforce the Lean Ladder to prevent over-engineering:
1. *Does this need to exist?*
2. *Is it already in this codebase?*
3. *Does stdlib do it?*
4. *Does a native platform feature cover it?*
5. *Does an installed dependency solve it?*
6. *Can it be one line?*
7. *Only then write the minimum code.*

## Coding Guidelines & Best Practices

- **Avoid Flaky Tests**: When creating or modifying configuration or integration tests in Rust that write temporary files:
  - Do not use plain system time nanos for temporary directories, as concurrent parallel test execution will cause name collisions.
  - Always use the atomic counter-backed `temp_dir()` helper (implemented with `TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed)`) to guarantee unique directories.
- **Git Commit Workflow**:
  - Preflight checks must pass before committing changes.
  - Commits can be initiated from the REPL via `/commit` or manually via standard git CLI.
- **Slash Commands**:
  - The CLI supports slash commands like `/steer <message>` and `/follow-up <message>` (along with Alt+Enter shortcut) for queuing turn inputs. Ensure all command specs are correctly updated if adding new CLI commands.
