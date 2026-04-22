# Chet

An AI-powered coding assistant for the terminal, built in Rust.

Chet talks to the Anthropic Messages API and uses tools to read, write, edit, search, and run commands in your codebase — all from a streaming CLI.

## Features

- **Streaming chat** — real-time SSE streaming from the Anthropic API
- **Built-in tools** — Read, Write, Edit, Bash, Glob, Grep, Subagent, MemoryRead, MemoryWrite
- **MCP servers** — connect external tool providers via JSON-RPC 2.0 over stdio; `/mcp reconnect` for resilient reconnection; binary content saved to disk
- **Agent loop** — automatic tool use cycles (Claude calls tools, gets results, continues)
- **Permission system** — permit/block/prompt rules, before/after hooks, HTTP webhook hooks, `--ludicrous` mode; compound commands matched per-subcommand; specificity-based evaluation (specific rules override general)
- **Session management** — auto-save, `--resume`, `-n`/`--name`, `/compact`, auto-compaction (80% threshold with circuit breaker), context tracking, auto-labeling
- **Prompt caching** — automatic cache control on system prompt and tool definitions
- **Extended thinking** — opt-in via `--thinking-budget` or `--effort` (low/medium/high/auto)
- **Streaming markdown** — bold, italic, headings, code blocks with syntax highlighting, lists, links, blockquotes, tables with box-drawing
- **Status line** — persistent bottom bar showing model, context usage, tokens, effort, session, plan mode, and active tool; updates in real-time during execution
- **Tool output polish** — spinner during API/tool execution, styled tool icons (⚡✓✗⊘), Ctrl+C returns to prompt
- **Subagents** — delegate complex sub-tasks to child agents that run silently and return results; supports `isolation: "worktree"` for parallel-safe execution
- **Retry & backoff** — automatic retry with exponential backoff and jitter for 429/529/5xx/network errors, respects `Retry-After` header
- **Provider abstraction** — `Provider` trait decouples the agent loop from any specific LLM API; ships with `AnthropicProvider`, `BedrockProvider` (feature-gated), and `VertexProvider` (feature-gated)
- **Plan mode** — `/plan` toggles read-only exploration mode (Read/Glob/Grep only), produces structured plans, approve/refine/discard workflow; `/plan fix the bug` enters with immediate prompt
- **Persistent memory** — global and per-project memory files loaded into system prompt, writable via tools, survives across sessions; `/memory` command to view/edit/reset
- **Line editor** — arrow keys, Home/End, word movement, history, tab completion for slash commands
- **REPL + print mode** — interactive or single-shot (`chet -p "explain this code"`)
- **Worktree isolation** — `--worktree` flag runs entire session in an isolated git worktree; subagents support `isolation: "worktree"` for conflict-free parallel execution; `/worktree exit` to return to original CWD
- **Parallel tool execution** — read-only tools (Read, Glob, Grep) run concurrently; failures isolated per-tool
- **CI/CD-friendly** — auto-detects piped output: no ANSI escapes, no spinner, plain tool events (`chet -p "..." | jq`); SIGHUP-safe session flush
- **TOML config** — `~/.chet/config.toml` for persistent settings
- **Single binary** — no runtime dependencies

## Installation

### Via cargo

```bash
cargo install --git https://github.com/scottyj503/chet
```

### Install script (Linux / macOS)

```bash
curl -sSf https://raw.githubusercontent.com/scottyj503/chet/main/scripts/install.sh | sh
```

### Docker

```bash
docker run --rm -e ANTHROPIC_API_KEY ghcr.io/scottyj503/chet -p "explain this code"
```

### GitHub Action

```yaml
- uses: scottyj503/chet/.github/actions/setup-chet@v1
  with:
    api-key: ${{ secrets.ANTHROPIC_API_KEY }}
- run: chet -p "review this PR"
```

### From source

```bash
git clone https://github.com/scottyj503/chet.git
cd chet
cargo install --path crates/chet-cli
```

## Quick Start

```bash
# Set your API key (or use ANTHROPIC_AUTH_TOKEN for Bearer auth)
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
      --auth-token <TOKEN>             Auth token for Bearer auth (overrides ANTHROPIC_AUTH_TOKEN)
      --resume <SESSION_ID>            Resume a previous session by ID or prefix
      --thinking-budget <TOKENS>       Enable extended thinking with token budget
      --effort <LEVEL>                 Set effort level (low, medium, high)
  -n, --name <NAME>                    Name for the session (overrides auto-labeling)
      --worktree                       Run in an isolated git worktree
      --worktree-branch <BRANCH>       Branch name for the worktree (implies --worktree)
      --ludicrous                      Skip all permission checks
      --provider <PROVIDER>             Provider: anthropic (default), bedrock, vertex
      --aws-region <REGION>             AWS region for Bedrock
      --vertex-project <PROJECT>        Google Cloud project for Vertex AI
      --vertex-region <REGION>          Google Cloud region for Vertex AI (default: us-east5)
      --verbose                        Enable debug logging
  -h, --help                           Print help
  -V, --version                        Print version
```

### Multi-Provider Support

```bash
# AWS Bedrock (requires --features bedrock)
cargo install --git https://github.com/scottyj503/chet --features bedrock
chet --provider bedrock --aws-region us-east-1

# Google Vertex AI (requires --features vertex)
cargo install --git https://github.com/scottyj503/chet --features vertex
chet --provider vertex --vertex-project my-project --vertex-region us-east5

# Claude Code compatible env vars
CLAUDE_CODE_USE_BEDROCK=1 chet
CHET_USE_VERTEX=1 GOOGLE_CLOUD_PROJECT=my-proj chet
```

### REPL Commands

| Command              | Description                              |
|----------------------|------------------------------------------|
| `/help`              | Show available commands                  |
| `/effort [level]`    | Show or set effort level (low, medium, high, auto) |
| `/plan [description]` | Toggle plan mode; with description, starts immediately |
| `/memory [subcommand]` | View/edit/reset persistent memory       |
| `/copy`              | Copy last response to clipboard          |
| `/mcp [reconnect]`   | Show MCP servers; reconnect by name      |
| `/model`             | Show current model (human-readable name) |
| `/cost`              | Show token usage                         |
| `/context`           | Show detailed context window usage       |
| `/compact`           | Compact conversation (archive + summarize) |
| `/sessions`          | List saved sessions                      |
| `/resume <prefix>`   | Resume a saved session by ID prefix      |
| `/worktree exit`     | Exit worktree, restore original CWD      |
| `/clear`             | Clear conversation (starts new session)  |
| `/quit`              | Exit                                     |

## Environment Variables

| Variable | Description | Required |
|----------|-------------|----------|
| `ANTHROPIC_API_KEY` | Your Anthropic API key | Yes (unless auth token or config) |
| `ANTHROPIC_AUTH_TOKEN` | Bearer token for LLM gateways/proxies (takes precedence over API key) | No |
| `CHET_MODEL` | Override default model | No |
| `ANTHROPIC_API_BASE_URL` | Custom API endpoint | No |
| `CHET_CONFIG_DIR` | Override config directory (default: `~/.chet/`) | No |

## Configuration

Create `~/.chet/config.toml`:

```toml
# memory_dir = "/custom/path/to/memory"  # default: ~/.chet/memory/

[api]
model = "claude-sonnet-4-5-20250929"
max_tokens = 64000
# api_key = "sk-ant-..."    # prefer ANTHROPIC_API_KEY env var
# auth_token = "token-..."  # prefer ANTHROPIC_AUTH_TOKEN env var (Bearer auth)
# thinking_budget = 10000  # enable extended thinking
# effort = "medium"        # effort level: low (1024), medium (8192), high (32768)

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

# HTTP hooks: POST JSON to a URL (2xx=approve, 403=deny)
# [[hooks]]
# event = "after_tool"
# command = "https://hooks.example.com/chet-events"
# timeout_ms = 10000

# MCP servers (external tool providers via JSON-RPC 2.0 over stdio)
[mcp.servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/home/user"]

[mcp.servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "ghp_xxxx" }
# timeout_ms = 30000  # default: 30 seconds
```

## Architecture

Chet is a Cargo workspace with focused crates:

| Crate | Purpose |
|-------|---------|
| `chet` | Binary: CLI entry, REPL, arg parsing |
| `chet-core` | Agent loop, conversation orchestration (provider-agnostic) |
| `chet-api` | Anthropic API client, SSE streaming, `AnthropicProvider` |
| `chet-tools` | Tool trait + built-in tools (Read, Write, Edit, Bash, Glob, Grep, MemoryRead, MemoryWrite); Subagent tool lives in chet-core |
| `chet-config` | Multi-tier TOML settings |
| `chet-bedrock` | AWS Bedrock provider (feature-gated: SigV4 signing, EventStream parser) |
| `chet-vertex` | Google Vertex AI provider (feature-gated: Google ADC, SSE reuse) |
| `chet-types` | Shared types, error hierarchy, `Provider` trait, SSE parser, Unicode-safe string utils |
| `chet-permissions` | Permission engine, rule matcher, hook runner |
| `chet-session` | Session persistence, context tracking, compaction |
| `chet-terminal` | Custom line editor, streaming markdown, syntax highlighting |
| `chet-mcp` | MCP client (JSON-RPC 2.0 over stdio, tool discovery, multi-server) |
| `chet-plugins` | Plugin system *(planned)* |
| `chet-lsp` | LSP client *(planned)* |
| `chet-sandbox` | Landlock/seccomp sandboxing *(planned)* |

## Building & Testing

```bash
# Check
cargo check --workspace

# Unit tests (494 tests — runs fast, no API key needed)
# 34 integration tests (ignored by default, run with --ignored)
cargo test --workspace

# Integration tests (6 SSE + 4 retry + 10 agent + 1 pipe mode + 3 MCP e2e + 3 session + 7 worktree — on-demand)
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
