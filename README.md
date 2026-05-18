<img src="./clawiev2.png" alt="Clawie" width="220" />

# Clawie by ShrimpAI

Clawie is a packaged workspace that keeps both sides of the project together:

- `rust-clawie` (main CLI/runtime side)
- `python-clawie` (Python mirror/workspace side)

This repository exists so the full project is shareable and runnable from one folder.

## Quick Start

1. From repository root, run:

```bash
./clawie
```

2. Work in these folders depending on focus:

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
