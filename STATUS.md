# Chet — Status Tracker

## Current Phase: Phase 10 COMPLETE — v1 shipped

## Phase Status

| Phase | Name | Status | Notes |
|-------|------|--------|-------|
| 0 | Scaffolding | **COMPLETE** | Workspace, types, CI |
| 1 | Minimal Streaming Chat | **COMPLETE** | SSE streaming, config, REPL |
| 2 | Tool System | **COMPLETE** | 6 tools, registry, agent loop |
| 3 | Permission System | **COMPLETE** | Rules, hooks, engine, CLI prompt |
| 4 | Session Management | **COMPLETE** | Session persistence, context tracking, compaction |
| 4.5 | Prompt Caching + Extended Thinking | **COMPLETE** | Cache control on system/tools, --thinking-budget flag, thinking block capture fix |
| 5a | Custom Line Editor | **COMPLETE** | crossterm raw mode, arrow keys, history, tab completion |
| 5b | Streaming Markdown Renderer | **COMPLETE** | syntect highlighting, line-level buffer, inline markdown, styled output |
| 5c | Tool Output Polish | **COMPLETE** | Spinner, styled tool icons, Ctrl+C cancellation, table rendering |
| 5d | Plan Mode | **COMPLETE** | Read-only agent mode, /plan command, plan file output, user approval gate |
| 6 | Subagent System | **COMPLETE** | SubagentTool, shared Arc<PermissionEngine>, builtins-only child, silent execution |
| 7 | Retry & Backoff | **COMPLETE** | Exponential backoff with jitter for 429/529/5xx/network errors, Retry-After header, config |
| 7.5 | Multi-Provider API | **COMPLETE** | Provider trait in chet-types, AnthropicProvider wraps ApiClient, Agent uses Arc<dyn Provider>, chet-core decoupled from chet-api |
| 8 | MCP Integration | **COMPLETE** | JSON-RPC 2.0 over stdio, multi-server, tool namespacing, /mcp command |
| 9 | Polish & Distribution | **COMPLETE** | Unicode-safe truncation, CWD fallback, O(n²) fix, auto-labels, descriptive permissions |
| 10 | Distribution | **COMPLETE** | Static binaries, Docker, install script, GitHub Action, crates.io |

### Phase 9 Checklist

- [x] Bounded memory for Bash tool output (cap/truncate large output) — already done (30KB cap)
- [x] Platform-correct temp dirs (`std::env::temp_dir()`, not hardcoded `/tmp`)
- [x] Release stream buffers in long sessions — acceptable (SSE buffer minor in practice)
- [x] Fix O(n²) message accumulation — `std::mem::take` instead of `messages.clone()`
- [x] Preserve plan mode + session titles through compaction — auto-label + label in summary
- [x] Handle CWD deletion in bash tool (fallback to ctx.cwd with warning)
- [x] Parallel file ops fail independently — already clean (sequential execution)
- [x] Preserve Unicode in Edit tool — already clean (Rust String throughout)
- [x] Validate permission match descriptions — descriptive messages with matched rule/args
- [x] Unicode-safe truncation — `truncate_str`/`truncate_string` across 11 sites

### Phase 10 Checklist

- [x] Static binary cross-compilation (Linux x86_64/aarch64 musl, macOS x86_64/aarch64)
- [x] GitHub Releases CI (tag → build → attach binaries + sha256sums)
- [x] Install script (`curl -sSf ... | sh`)
- [x] Docker image (Alpine-based, ~15MB)
- [x] `cargo install --git` (package renamed, crates.io metadata ready)
- [x] GitHub Action (`uses: scottyj503/chet/.github/actions/setup-chet@v1`)
- [x] Pure-Rust dependencies (regex-fancy, rustls — no C cross-compilation issues)
- [x] Library crates marked `publish = false`

## Completed Tasks

- Phase 0: Cargo workspace with 13 crates, shared types (Message, ContentBlock, Tool trait, error hierarchy), CI pipeline, cargo-deny
- Phase 1: chet-api (SSE streaming client), chet-config (TOML settings, API key), chet-cli (clap args, REPL, print mode, slash commands)
- Phase 2: chet-tools (6 built-in tools: Read, Write, Edit, Bash, Glob, Grep), tool registry, chet-core (agent loop with tool use cycles)
- Phase 3: chet-permissions (permission engine, rule matcher, hook runner, prompt handler), config integration, agent integration, --ludicrous CLI flag
- Phase 4: chet-session (Session/SessionStore/ContextTracker/compact), JSON persistence in ~/.chet/sessions/, --resume flag, /context /compact /sessions /resume commands, auto-save after each turn, context line display
- Phase 4.5: Prompt caching (CacheControl on system prompt + last tool definition, always on), extended thinking (--thinking-budget flag, ThinkingConfig, thinking block capture bug fix)
- Live API testing: Validated all phases against real Anthropic API, fixed 2 bugs, added integration test suite
- Phase 5a: Custom line editor (chet-terminal crate) — crossterm raw mode, LineBuffer with cursor, History with file persistence, SlashCommandCompleter, TerminalRenderer, panic hook for raw mode safety
- Phase 5b: Streaming markdown renderer — StreamingMarkdownRenderer (line buffer + inline parse + state machine), CodeHighlighter (syntect), style helpers, tool events moved to stderr. Deferred: table rendering (needs full buffering), spinners/Ctrl+C/tool colors (Phase 5c)
- Phase 5c: Tool output polish — styled tool events (⚡✓✗⊘ icons with colors), braille spinner during API/tool execution, Ctrl+C cancellation via CancellationToken (returns to prompt), markdown table rendering with box-drawing characters and alignment
- Phase 5d: Plan mode — `/plan` command toggles read-only mode (Read/Glob/Grep only), plan system prompt, plan file output to `~/.chet/plans/`, approval gate (approve/refine/discard), `pop_last_turn()` for discard, dynamic `plan> ` prompt
- Phase 6: Subagent system — `SubagentTool` in chet-core, `Agent` takes `Arc<PermissionEngine>` (shared between parent/child), child gets builtins-only registry (no SubagentTool → no recursion), runs silently with no-op event callback, extracts last assistant text as tool result
- Phase 7: Retry & backoff — `RetryConfig` + `is_retryable()` + `calculate_delay()` in `chet-api/src/retry.rs`, retry loop in `ApiClient::create_message_stream()`, `parse_retry_after()` for server-specified delays, ±25% jitter, `[api.retry]` config section, defaults: 2 retries / 1s initial / 2x factor / 60s max
- Phase 7.5: Multi-Provider API — `Provider` trait + `EventStream` type alias in `chet-types/src/provider.rs`, `AnthropicProvider` in `chet-api/src/provider.rs` wraps `ApiClient`, `Agent` + `SubagentTool` take `Arc<dyn Provider>`, `chet-core` no longer depends on `chet-api` (only dev-dependency for tests)
- Phase 8: MCP Integration — Custom JSON-RPC 2.0 over stdio transport, `McpClient` (initialize handshake + tool discovery + tool call), `McpTool` (implements `Tool` trait with `mcp__server__tool` namespacing), `McpManager` (multi-server orchestration with graceful failure), `[mcp.servers.*]` TOML config, `/mcp` slash command, MCP info in startup banner
- Phase 9: Polish & Distribution — Unicode-safe `truncate_str`/`truncate_string` (11 sites), platform-correct temp dirs, CWD deletion fallback in bash tool, O(n²) `messages.clone()` → `std::mem::take`, auto-label sessions + preserve through compaction, descriptive permission match messages with rule/args context
- Phase 10: Distribution — Pure-Rust deps (regex-fancy, rustls), package rename chet-cli→chet with crates.io metadata, release CI (4-target matrix + GitHub Release + Docker push), install script, Dockerfile (Alpine), GitHub Action (setup-chet), library crates `publish = false`

## Test Summary

- 333 unit tests passing (34 api, 8 config, 16 core/agent+subagent+worktree, 21 tools, 31 permissions, 30 session, 20 types, 135 terminal, 9 cli, 29 mcp)
  - 7 additional ignored tests (worktree: require git + filesystem, run with `--ignored`)
- 6 integration tests (mock SSE pipeline, run with `cargo test -- --ignored`)
- 4 cancellation integration tests (MockProvider + SlowTool, run with `cargo test -p chet-core --test cancellation_integration -- --ignored`)
- Zero clippy warnings
- `cargo run --bin chet -- --help` and `--version` working

## Live API Testing (2026-02-18)

All Phases 0-4.5 validated against live Anthropic API:
- **Streaming chat**: SSE streaming, response parsing, token tracking — PASS
- **Tool use (Read)**: Agent loop calls Read tool, returns result, gets final answer — PASS
- **Tool use (Bash)**: Runs shell commands, captures output — PASS
- **Tool use (Grep)**: Regex search, files_with_matches mode — PASS
- **Prompt caching**: cache_write on first call, cache_read on subsequent — PASS
- **Extended thinking**: --thinking-budget flag, thinking blocks streamed to stderr — PASS
- **REPL mode**: /help, /cost, /context, /quit all working — PASS
- **Session save**: Auto-save after each turn, /sessions lists saved — PASS
- **Session resume**: --resume with prefix matching, conversation history preserved — PASS

Bugs found and fixed:
- **MessageStream event drop**: When SSE parser returned multiple events from one byte chunk, only the first was yielded — rest silently dropped. Fixed by buffering parsed events in `pending_events` Vec.
- **Usage deserialization**: `input_tokens` and `output_tokens` lacked `#[serde(default)]`, causing parse failures on `message_delta` events that only include `output_tokens`.

## Open Blockers

- (none)

## Future Test Items

- ~~**Cancellation integration test**~~: **DONE** — 4 `#[ignore]` tests in `crates/chet-core/tests/cancellation_integration.rs` (MockProvider + SlowTool). Covers mid-stream, mid-tool, after-completion, and pre-cancelled token. Run with `cargo test -p chet-core --test cancellation_integration -- --ignored`.
- **Subagent end-to-end**: Parent agent spawns child via MockProvider, child runs a tool, parent gets text result. Validates SubagentTool → Agent → tool → result pipeline. Reuse MockProvider infrastructure.
- **Retry/backoff**: Retry lives in `ApiClient`, not the `Provider` trait, so MockProvider can't exercise it directly. Needs a `MockProvider` that returns retryable `ApiError`s on first N calls then succeeds, or a mock at the `ApiClient` level. Verify retry with delay then success is transparent to agent.
- **Multi-tool-use turn**: MockProvider returns 2+ tool_use blocks in one response, verify all execute and results sent back. Common real-world pattern with no test coverage.
- **Plan mode tool blocking**: Agent in read-only mode, MockProvider requests Write tool. Verify ToolBlocked event fires. Validates safety net.
- **Non-interactive pipe mode**: Agent with TTY=false, verify no ANSI escapes, silent spinner, plain markdown output.
- **Session round-trip**: Save session after agent run, load back, verify messages intact. Filesystem integration test.
- **MCP end-to-end**: Spawn real MCP server process, connect, discover tools, call one. Validates full JSON-RPC handshake.
- **Compaction state preservation**: Run agent, set label, compact, verify label and plan mode survive through compaction.

## Product Direction

- **CI/CD-first agent**: Leverage compiled Rust binary for zero-dependency, fast-startup headless agent mode targeting CI/CD pipelines (PR review, code checks, test generation). Key differentiator vs Node.js-based tools.

## Post-v1

- **LSP Client**: Opt-in (default off), --lsp flag or [lsp] config. Grep/Read cover 90% of needs; LSP is heavyweight (1-2GB RAM) and hurts CI/CD. Revisit if users request it. Filter gitignored files from results.
- ~~**Worktree isolation**~~: **DONE** — `--worktree` flag + subagent `isolation: "worktree"` for parallel agents in isolated git worktrees. `WorktreeCreate`/`WorktreeRemove` hook events, RAII cleanup via `ManagedWorktree`.
- ~~**Non-interactive mode optimization**~~: **DONE** — TTY detection via `std::io::IsTerminal`, plain markdown passthrough, silent spinner, plain tool events, no ANSI in piped output.
- **ConfigChange hook event**: Fire hook when config files change during a session. Enables hot-reload without restart.
- **File-not-found path suggestions**: When model drops the repo prefix from a path, suggest the corrected path. Saves wasted agent turns.
- **Enhanced permission restriction reasons**: Show why a path or working directory is blocked, not just that it is.
- **Status line**: Persistent terminal status bar showing model, tokens, cost, session ID, mode (plan/normal), active agent name (e.g., `subagent: code-quality-reviewer`), active MCP server+tool (e.g., `mcp: jira → search_issues`), LSP status. Structured JSON output for CI/CD log parsing.
- **Memory management**: Clear internal caches after compaction, cap file history snapshots, free completed task output. Prevent unbounded growth in long sessions.
- **`chet agents` CLI command**: List all configured agents/subagent definitions for discoverability.
- **MCP reconnect resilience**: Handle `/mcp reconnect` with non-existent server name gracefully instead of freezing.
- **Session flush on disconnect**: Flush session data before hooks/analytics on SSH disconnect or connection drop. Critical for remote/CI usage.

## Decisions Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-02-17 | Project name: Chet | Original name, no branding conflicts |
| 2026-02-17 | Config format: TOML | Idiomatic Rust, human-readable |
| 2026-02-17 | Config dir: ~/.chet/ | Own identity, no conflicts |
| 2026-02-17 | 13-crate workspace | Clean separation of concerns |
| 2026-02-17 | Tool trait: Pin<Box<dyn Future>> | dyn-compatible; ToolContext passed by value |
| 2026-02-17 | SSE parser: custom incremental | No good Rust SSE crate for our needs |
| 2026-02-17 | License: MIT | Simple, maximally permissive, Rust ecosystem standard |
| 2026-02-17 | Session persistence: JSON files | One file per session in ~/.chet/sessions/, atomic write |
| 2026-02-17 | Token estimation: chars/4 | Simple heuristic, no tokenizer dependency |
| 2026-02-17 | Compaction: user-triggered only | /compact command, no automatic truncation |
| 2026-02-17 | Session IDs: UUID with prefix matching | --resume a1b2c3 matches, errors if ambiguous |
| 2026-02-18 | Live API testing before Phase 5 | Validate plumbing before building rich UI |
| 2026-02-18 | Phase 5 split into 5a/5b/5c | Line editor, markdown renderer, tool output polish |
| 2026-02-18 | Reorder: subagents moved from 10→6 | No deps on phases 7-9; enables CI/CD direction early; LSP moved to 10 (independent, lower priority) |
| 2026-02-18 | MCP: roll our own JSON-RPC | Protocol is ~5 message types; avoids large rmcp dependency tree |
| 2026-02-18 | MCP: stdio transport only | Most servers are local processes; HTTP transport can come later |
| 2026-02-18 | MCP: eager startup, no reconnect | Start all at session init; if a server dies, its tools are dead for the session |
| 2026-02-18 | MCP: mcp__server__tool namespacing | Prevents collisions between built-in tools and MCP tools from different servers |
| 2026-02-18 | Drop Plugin System phase | MCP covers external tool extensibility; plugins deferred until concrete need arises |
| 2026-02-19 | Defer LSP Client to post-v1 | Grep/Read cover 90% of needs; LSP is heavyweight and hurts CI/CD story; revisit if users request |
| 2026-02-19 | Custom floor_char_boundary | MSRV 1.85 vs floor_char_boundary stable 1.91; manual impl using is_char_boundary |
| 2026-02-19 | O(n²) fix: std::mem::take | Move messages into request, restore after API call — O(1) vs clone's O(n) per iteration |
