# Chet — Status Tracker

## Current Phase: 4.5 — Prompt Caching + Extended Thinking (COMPLETE)

## Phase Status

| Phase | Name | Status | Notes |
|-------|------|--------|-------|
| 0 | Scaffolding | **COMPLETE** | Workspace, types, CI |
| 1 | Minimal Streaming Chat | **COMPLETE** | SSE streaming, config, REPL |
| 2 | Tool System | **COMPLETE** | 6 tools, registry, agent loop |
| 3 | Permission System | **COMPLETE** | Rules, hooks, engine, CLI prompt |
| 4 | Session Management | **COMPLETE** | Session persistence, context tracking, compaction |
| 4.5 | Prompt Caching + Extended Thinking | **COMPLETE** | Cache control on system/tools, --thinking-budget flag, thinking block capture fix |
| 5 | Rich Terminal UI | Not started | Custom line editor (crossterm raw mode), real-time spell checking, input history |
| 6 | Multi-Provider API | Not started | |
| 7 | LSP Client | Not started | |
| 8 | MCP Integration | Not started | Lazy-load MCP servers on demand |
| 9 | Plugin System | Not started | |
| 10 | Subagent System | Not started | Lazy-load subagents on demand |
| 11 | Bash Sandboxing | Not started | |
| 12 | Polish & Distribution | Not started | |

## Completed Tasks

- Phase 0: Cargo workspace with 13 crates, shared types (Message, ContentBlock, Tool trait, error hierarchy), CI pipeline, cargo-deny
- Phase 1: chet-api (SSE streaming client), chet-config (TOML settings, API key), chet-cli (clap args, REPL, print mode, slash commands)
- Phase 2: chet-tools (6 built-in tools: Read, Write, Edit, Bash, Glob, Grep), tool registry, chet-core (agent loop with tool use cycles)
- Phase 3: chet-permissions (permission engine, rule matcher, hook runner, prompt handler), config integration, agent integration, --ludicrous CLI flag
- Phase 4: chet-session (Session/SessionStore/ContextTracker/compact), JSON persistence in ~/.chet/sessions/, --resume flag, /context /compact /sessions /resume commands, auto-save after each turn, context line display
- Phase 4.5: Prompt caching (CacheControl on system prompt + last tool definition, always on), extended thinking (--thinking-budget flag, ThinkingConfig, thinking block capture bug fix)

## Test Summary

- 82 tests passing (5 SSE parser, 4 config, 19 tools, 24 permissions, 23 session, 7 message types)
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

Bug found and fixed:
- **MessageStream event drop**: When SSE parser returned multiple events from one byte chunk, only the first was yielded — rest silently dropped. Fixed by buffering parsed events in `pending_events` Vec.

## Open Blockers

- (none)

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
