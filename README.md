<img src="./clawiev2.png" alt="Clawie" width="220" />

# Clawie by ShrimpAI

Clawie is a packaged workspace that keeps both sides of the project together:

- `rust-clawie` (main CLI/runtime side)
- `python-clawie` (Python mirror/workspace side)

This repository exists so the full project is shareable and runnable from one folder.

## Key Features & Recent Updates

### 1. Interactive Setup Wizard
Configure your environment variables, provider, model, API key, and base URL interactively:
```bash
./clawie setup
```
Settings are persisted in `settings.json` under the Clawie config directory.

### 2. Lazy Senior Dev Mode (Lean Mode)
Clawie enforces a **Lean Ladder** to prevent over-engineering:
1. *Does this need to exist?*
2. *Is it already in this codebase?*
3. *Does stdlib do it?*
4. *Does a native platform feature cover it?*
5. *Does an installed dependency solve it?*
6. *Can it be one line?*
7. *Only then write the minimum code.*

Manage this mode directly inside the REPL session:
- `/lean [lite|full|ultra|off]`: Switch or view the active lean mode (default is `full`).
- `/lean-review`: Review current diff for over-engineering.
- `/lean-audit`: Scan repository for over-engineering.
- `/lean-debt`: Harvest `clawie:` simplification comments into a ledger.
- `/lean-gain`: Show benchmark impact metrics.
- `/lean-help`: Print command reference.

### 3. Repository Mapping
Use the `/map` (or `/repo-map`) command in the REPL to generate a ranked map of the repository's files and extracted symbols, helping navigate large codebases.

### 4. Git Integration
Manage commits directly from the REPL:
- `/commit`: Preflight checks changes, generates a commit message, and commits them.
- `/undo`: Undoes the last commit (soft reset, keeping changes).

### 5. Workspace RAG Service (`claw-rag-service`)
SQLite-backed vector indexing service for semantic repository searches:
- **Ingest files**: `cargo run -p claw-rag-service -- ingest --workspace .`
- **Serve API & UI**: `cargo run -p claw-rag-service -- serve`

## Quick Start

1. Set up the workspace:
```bash
./clawie setup
```

2. Launch the Clawie agent REPL:
```bash
./clawie
```

3. Work in these folders depending on focus:

- `rust-clawie` for CLI/runtime behavior
- `python-clawie` for Python-side mirrored modules and tooling

## Repository Layout

```text
Clawie/
├── README.md
├── clawie
├── rust-clawie/
└── python-clawie/
```

## Long Coding Sessions (Improved)

The runtime defaults were increased to better support longer sessions:

- `max_turns`: `64` (was lower)
- `max_budget_tokens`: `12000` (was lower)
- `compact_after_turns`: `48`
- `turn-loop --max-turns`: default `12`

You can tune these at runtime with environment variables:

```bash
export CLAWIE_MAX_TURNS=120
export CLAWIE_MAX_BUDGET_TOKENS=30000
export CLAWIE_COMPACT_AFTER_TURNS=80
export CLAWIE_STRUCTURED_OUTPUT=false
export CLAWIE_STRUCTURED_RETRY_LIMIT=2
./clawie
```

Notes:

- Invalid values fall back to defaults.
- Numeric values are clamped to at least `1`.

## Useful Commands

Run from repository root.

```bash
# Python-side summary
python3 -m python-clawie.src.main summary

# Run a stateful loop with explicit turn count
python3 -m python-clawie.src.main turn-loop "audit this module" --max-turns 30

# Resume an existing session
python3 -m python-clawie.src.main resume-session <session_id> "continue"
```

## Product Naming

- `Clawie`: product name
- `ShrimpAI`: parent brand
- `Jameclaw`: legacy/origin naming context

## Why This Package Exists

Earlier working copies were split across multiple local folders. This package keeps everything in one Git-ready structure so onboarding, development, and sharing are simpler.

<img src="./ShrimpAIR.png" alt="ShrimpAI mascot" width="260" />
