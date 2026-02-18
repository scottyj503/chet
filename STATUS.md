# Chet — Status Tracker

## Current Phase: Phase 7.5 COMPLETE — Ready for Phase 8 (MCP Integration)

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
| 8 | MCP Integration | Not started | Lazy-load MCP servers on demand |
| 9 | Plugin System | Not started | Hot-reload: plugins available immediately without restart |
| 10 | LSP Client | Not started | |
| 11 | Bash Sandboxing | Not started | |
| 12 | Polish & Distribution | Not started | Bounded memory for Bash tool output, platform-correct temp dirs |

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

## Test Summary

- 258 unit tests passing (34 api (10 SSE/stream + 12 retry + 8 client + 1 classify + 3 provider), 6 config, 14 core/agent+subagent, 20 tools, 24 permissions, 23 session, 9 types (7 message + 2 provider), 124 terminal, 9 cli)
- 6 integration tests (mock SSE pipeline, run with `cargo test -- --ignored`)
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

- **Cancellation integration test**: End-to-end test of `Agent::run()` with a `CancellationToken` cancelled mid-stream and mid-tool. Needs a mock HTTP server (e.g. `wiremock`) to stream SSE slowly. Can share infrastructure with Phase 6 rate-limit/retry tests.

## Product Direction

- **CI/CD-first agent**: Leverage compiled Rust binary for zero-dependency, fast-startup headless agent mode targeting CI/CD pipelines (PR review, code checks, test generation). Key differentiator vs Node.js-based tools.

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
