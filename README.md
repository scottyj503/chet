# Chet

An AI-powered coding assistant for the terminal, built in Rust.

Chet talks to the Anthropic Messages API and uses tools to read, write, edit, search, and run commands in your codebase — all from a streaming CLI.

## Features

- **Streaming chat** — real-time SSE streaming from the Anthropic API
- **Built-in tools** — Read, Write, Edit, Bash, Glob, Grep, Subagent
- **Agent loop** — automatic tool use cycles (Claude calls tools, gets results, continues)
- **Permission system** — permit/block/prompt rules, before/after hooks, `--ludicrous` mode
- **Session management** — auto-save, `--resume`, `/compact`, context tracking
- **Prompt caching** — automatic cache control on system prompt and tool definitions
- **Extended thinking** — opt-in via `--thinking-budget`
- **Streaming markdown** — bold, italic, headings, code blocks with syntax highlighting, lists, links, blockquotes, tables with box-drawing
- **Tool output polish** — spinner during API/tool execution, styled tool icons (⚡✓✗⊘), Ctrl+C returns to prompt
- **Subagents** — delegate complex sub-tasks to child agents that run silently and return results
- **Retry & backoff** — automatic retry with exponential backoff and jitter for 429/529/5xx/network errors, respects `Retry-After` header
- **Provider abstraction** — `Provider` trait decouples the agent loop from any specific LLM API; ships with `AnthropicProvider`
- **Plan mode** — `/plan` toggles read-only exploration mode (Read/Glob/Grep only), produces structured plans, approve/refine/discard workflow
- **Line editor** — arrow keys, Home/End, word movement, history, tab completion for slash commands
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
  -p, --print <PROMPT>                 Send a single prompt and print the response
      --model <MODEL>                  Model to use (default: claude-sonnet-4-5-20250929)
      --max-tokens <MAX_TOKENS>        Maximum tokens in the response
      --api-key <API_KEY>              API key (overrides ANTHROPIC_API_KEY)
      --resume <SESSION_ID>            Resume a previous session by ID or prefix
      --thinking-budget <TOKENS>       Enable extended thinking with token budget
      --ludicrous                      Skip all permission checks
      --verbose                        Enable debug logging
  -h, --help                           Print help
  -V, --version                        Print version
```

### REPL Commands

| Command              | Description                              |
|----------------------|------------------------------------------|
| `/help`              | Show available commands                  |
| `/plan`              | Toggle plan mode (read-only exploration) |
| `/model`             | Show current model                       |
| `/cost`              | Show token usage                         |
| `/context`           | Show detailed context window usage       |
| `/compact`           | Compact conversation (archive + summarize) |
| `/sessions`          | List saved sessions                      |
| `/resume <prefix>`   | Resume a saved session by ID prefix      |
| `/clear`             | Clear conversation (starts new session)  |
| `/quit`              | Exit                                     |

## Environment Variables

| Variable | Description | Required |
|----------|-------------|----------|
| `ANTHROPIC_API_KEY` | Your Anthropic API key | Yes (unless in config or `--api-key`) |
| `CHET_MODEL` | Override default model | No |
| `ANTHROPIC_API_BASE_URL` | Custom API endpoint | No |
| `CHET_CONFIG_DIR` | Override config directory (default: `~/.chet/`) | No |

## Configuration

Create `~/.chet/config.toml`:

```toml
[api]
model = "claude-sonnet-4-5-20250929"
max_tokens = 16384
# api_key = "sk-ant-..."  # prefer ANTHROPIC_API_KEY env var
# thinking_budget = 10000  # enable extended thinking

# [api.retry]
# max_retries = 2          # default: 2 (0 disables retries)
# initial_delay_ms = 1000  # default: 1000
# max_delay_ms = 60000     # default: 60000

[[permissions.rules]]
tool = "Read"
level = "permit"

[[permissions.rules]]
tool = "Bash"
args = "command:rm *"
level = "block"

[[hooks]]
event = "before_tool"
command = "/usr/local/bin/audit.sh"
timeout_ms = 5000
```

## Architecture

Chet is a Cargo workspace with focused crates:

| Crate | Purpose |
|-------|---------|
| `chet-cli` | Binary: CLI entry, REPL, arg parsing |
| `chet-core` | Agent loop, conversation orchestration (provider-agnostic) |
| `chet-api` | Anthropic API client, SSE streaming, `AnthropicProvider` |
| `chet-tools` | Tool trait + built-in tools (Read, Write, Edit, Bash, Glob, Grep); Subagent tool lives in chet-core |
| `chet-config` | Multi-tier TOML settings |
| `chet-types` | Shared types, error hierarchy, `Provider` trait |
| `chet-permissions` | Permission engine, rule matcher, hook runner |
| `chet-session` | Session persistence, context tracking, compaction |
| `chet-terminal` | Custom line editor, streaming markdown, syntax highlighting |
| `chet-mcp` | MCP client *(planned)* |
| `chet-plugins` | Plugin system *(planned)* |
| `chet-lsp` | LSP client *(planned)* |
| `chet-sandbox` | Landlock/seccomp sandboxing *(planned)* |

## Building & Testing

```bash
# Check
cargo check --workspace

# Unit tests (258 tests — runs fast, no API key needed)
cargo test --workspace

# Integration tests (6 tests — mock SSE pipeline, on-demand)
cargo test --workspace -- --ignored

# All tests
cargo test --workspace -- --include-ignored

# Clippy
cargo clippy --workspace

# Release build
cargo build --release --bin chet
```

## License

MIT
