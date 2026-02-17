# Chet

An AI-powered coding assistant for the terminal, built in Rust.

Chet talks to the Anthropic Messages API and uses tools to read, write, edit, search, and run commands in your codebase — all from a streaming CLI.

## Features

- **Streaming chat** — real-time SSE streaming from the Anthropic API
- **Built-in tools** — Read, Write, Edit, Bash, Glob, Grep
- **Agent loop** — automatic tool use cycles (Claude calls tools, gets results, continues)
- **REPL + print mode** — interactive or single-shot (`chet -p "explain this code"`)
- **TOML config** — `~/.chet/config.toml` for persistent settings
- **Single binary** — no runtime dependencies

## Quick Start

```bash
# Build from source
cargo install --path crates/chet-cli

# Set your API key
export ANTHROPIC_API_KEY=sk-ant-...

# Interactive mode
chet

# Single prompt
chet -p "What does this project do?"
```

## Usage

```
chet [OPTIONS]

Options:
  -p, --print <PROMPT>           Send a single prompt and print the response
      --model <MODEL>            Model to use (default: claude-sonnet-4-5-20250929)
      --max-tokens <MAX_TOKENS>  Maximum tokens in the response
      --api-key <API_KEY>        API key (overrides ANTHROPIC_API_KEY)
      --verbose                  Enable debug logging
  -h, --help                     Print help
  -V, --version                  Print version
```

### REPL Commands

| Command  | Description              |
|----------|--------------------------|
| `/help`  | Show available commands  |
| `/model` | Show current model       |
| `/cost`  | Show token usage         |
| `/clear` | Clear conversation       |
| `/quit`  | Exit                     |

## Configuration

Create `~/.chet/config.toml`:

```toml
[api]
model = "claude-sonnet-4-5-20250929"
max_tokens = 16384
# api_key = "sk-ant-..."  # prefer ANTHROPIC_API_KEY env var
```

## Architecture

Chet is a Cargo workspace with focused crates:

| Crate | Purpose |
|-------|---------|
| `chet-cli` | Binary: CLI entry, REPL, arg parsing |
| `chet-core` | Agent loop, conversation orchestration |
| `chet-api` | Anthropic API client, SSE streaming |
| `chet-tools` | Tool trait + built-in tools (Read, Write, Edit, Bash, Glob, Grep) |
| `chet-config` | Multi-tier TOML settings |
| `chet-types` | Shared types, error hierarchy |
| `chet-permissions` | Hook system, permit/block/prompt rules *(planned)* |
| `chet-session` | Persistence, context windowing *(planned)* |
| `chet-terminal` | Streaming markdown renderer *(planned)* |
| `chet-mcp` | MCP client *(planned)* |
| `chet-plugins` | Plugin system *(planned)* |
| `chet-lsp` | LSP client *(planned)* |
| `chet-sandbox` | Landlock/seccomp sandboxing *(planned)* |

## Building

```bash
# Check
cargo check --workspace

# Test (26 tests)
cargo test --workspace

# Clippy
cargo clippy --workspace

# Release build
cargo build --release --bin chet
```

## License

MIT
