# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**term-ai** is a Rust CLI tool that queries a local Ollama AI server to generate shell commands from natural language prompts. It targets macOS 15+ users and responds with only shell commands (no prose). Requires a running Ollama instance.

## Build & Development Commands

```bash
cargo build                # Debug build
cargo build --release      # Optimized release build (opt-level 3, LTO, stripped)
cargo run -- "prompt"      # Run with CLI argument
echo "prompt" | cargo run  # Run with stdin
cargo test                 # Run all unit tests
cargo clippy               # Lint
cargo fmt                  # Format code
cargo fmt --check          # Check formatting without modifying
```

The release profile uses aggressive optimizations: `opt-level = 3`, `lto = true`, `codegen-units = 1`, `strip = true`.

## Architecture

All logic lives in a single file: `src/main.rs`. The design is intentionally monolithic for simplicity.

**Key components:**

- **`Args`** — Clap-derived struct for CLI parsing. Positional `prompt` arg, `--model` (default: `llama3.2`), `--endpoint` (default: `http://localhost:11434`)
- **`build_prompt()`** — Wraps user input with hardcoded system instructions that constrain Ollama to output only shell commands, prefer Homebrew, and avoid destructive operations
- **`call_ollama()`** — Synchronous HTTP POST to Ollama's `/api/generate` endpoint using `reqwest::blocking`
- **`get_user_prompt()`** — Implements input precedence: CLI argument > stdin > error

**Data flow:** Parse args → get prompt → build full prompt with system instructions → call Ollama API → print response to stdout

**Error handling:** Functions return `Result<T, Box<dyn std::error::Error>>`. Errors print to stderr and exit with code 1.

## Key Design Decisions

- **Synchronous HTTP** via `reqwest::blocking` rather than async — simpler for a CLI that blocks on a single request. `tokio` is included as a transitive dependency but not directly used for async orchestration.
- **Hardcoded system prompt** ensures safety constraints (no `rm -rf`, no destructive ops) cannot be bypassed by user input.
- **Input precedence:** CLI argument takes priority over stdin. Both empty results in an error.
