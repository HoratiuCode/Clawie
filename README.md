<img src="./clawiev2.png" alt="clawiev2" width="220" />

# Clawie by ShrimpAI

Clawie by ShrimpAI is a combined repository that packages the two parts of the Clawie project in one place.
Jameclaw is treated here as the legacy/origin name for earlier local project layouts.

- `rust-clawie`
- `python-clawie`

The goal of this repository is simple: instead of sharing separate folders from different places on one machine, everything needed for the full Clawie project is grouped into one directory that can be uploaded to GitHub and shared with other users.

## Quick Start

If you are opening Clawie for the first time:

1. Run `./clawie` from this repository root.
2. It opens the Rust interactive shell directly by default.
3. Open `rust-clawie` when you want the main interactive shell sources.
4. Open `python-clawie` when you want the Python workspace and mirrored project files.
5. Use `/search` inside the shell to find files or text in the workspace.
6. Use `/move <source> <destination>` to move a file or folder to another path.

## What Clawie Is

Clawie is the primary product name.
ShrimpAI is the parent brand behind this packaged repository.
Jameclaw is the legacy/origin name kept only as historical context where needed.

The Rust side is the main interactive CLI experience. It is responsible for:

- startup and onboarding
- provider and API key setup
- model selection
- chat and request handling
- slash commands such as `/help` and `/model`
- the terminal user experience

The Python side contains the related project workspace and source tree that lives alongside the CLI. In this combined package, both parts are preserved so the full project can be explored, extended, and shared together.

## Repository Layout

```text
Clawie-full/
├── README.md
├── .gitignore
├── rust-clawie/
└── python-clawie/
```

### `rust-clawie`

This is the Rust application side of Clawie.

It contains the code for the main command-line program, including the terminal chat loop, onboarding flow, command handling, and runtime behavior.

If someone wants to work on the CLI itself, this is the main folder they should open first.

### `python-clawie`

This is the Python-side Clawie project.

It contains the Python workspace and related project files that belong to the broader Clawie setup.

If someone wants to inspect or extend the Python part of the project, this is the folder they should use.

## How The Two Parts Fit Together

This repository does not force the Rust and Python parts into one codebase. Instead, it keeps them side by side.

That makes the structure easier to understand:

- the Rust folder is for the interactive CLI
- the Python folder is for the Python project
- the root folder is only the wrapper that makes the whole package easy to publish and share

This is useful because the original working copies came from different locations on the local machine. In `Clawie-full`, they are organized into one clean package.

<img src="./ShrimpAIR.png" alt="ShrimpAI mascot" width="260" />

## How To Use This Repository

After cloning or downloading the repository:

1. Open the root folder.
2. Run `./clawie` for the default launcher flow.
3. Enter `rust-clawie` if your focus is the terminal app and CLI behavior.
4. Enter `python-clawie` if your focus is the Python project.

For most users, the best default is `./clawie`, because that opens the interactive Clawie application directly with no intermediate menu.

## Documentation And Localization

Clawie keeps its shared documentation at the repository root so the package is easy to understand from one place.

- `README.md` is the main entry point for the packaged repository.
- `rust-clawie` contains the interactive CLI and its startup/onboarding flow.
- `python-clawie` contains the mirrored Python workspace and reference project files.

If you add user-facing text, prefer keeping the launcher labels and docs consistent so onboarding stays easy to follow.

## Search And File Moves

Clawie is meant to help a computer user inspect and reorganize a workspace without leaving the terminal.

- Use `/search <query> [path]` to look through filenames in the current workspace.
- Use `/move <source> <destination>` to move a file or folder.
- Use `/ps` to show the hidden session and workspace summary.
- Use `/cost` to review token usage and estimated API cost.
- Use `/reload` to reload config and refresh the active model client.

## Why This Combined Repository Exists

This repository exists to solve a packaging problem.

Originally, the full Clawie project was split across different local folders. That made it harder to upload, document, and share. `Clawie-full` gives one place to keep:

- the Rust CLI project
- the Python project
- one root README
- one root `.gitignore`

This makes GitHub publishing simpler and makes the project easier for other users to understand.

## What Was Cleaned Before Packaging

This combined package was prepared to be safer for publishing.

Local-only files were excluded or removed, including:

- build output such as `target`
- machine-specific metadata such as `.DS_Store`
- local tool folders such as `.claw` and `.claude`
- image and screenshot assets removed from this packaged copy

This helps keep the repository cleaner and avoids uploading unnecessary local or generated files.

## What To Upload

If you want one GitHub repository that contains the full Clawie package, upload this folder:

`Clawie-full`

That is the folder intended for sharing.

## Notes

This root README is the only README kept in the packaged repository.

That was done intentionally so the shared version is easier to understand from the top level without duplicated documentation spread across multiple nested folders.
