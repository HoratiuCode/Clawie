<img src="./clawiev2.png" alt="clawiev2" width="220" />

# Clawie by ShrimpAI

Clawie by ShrimpAI is a combined repository that packages the two parts of the Clawie project in one place.
Jameclaw is treated here as the legacy/origin name for earlier local project layouts.

- `rust-clawie`
- `python-clawie`

The goal of this repository is simple: instead of sharing separate folders from different places on one machine, everything needed for the full Clawie project is grouped into one directory that can be uploaded to GitHub and shared with other users.

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
2. Choose which part you want to work on.
3. Enter `rust-clawie` if your focus is the terminal app and CLI behavior.
4. Enter `python-clawie` if your focus is the Python project.

For most users, the main starting point is `rust-clawie`, because that is where the interactive Clawie application lives.

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
