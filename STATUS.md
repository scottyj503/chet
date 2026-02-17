# Chet — Status Tracker

## Current Phase: 2 — Tool System (COMPLETE, moving to Phase 3)

## Phase Status

| Phase | Name | Status | Notes |
|-------|------|--------|-------|
| 0 | Scaffolding | **COMPLETE** | Workspace, types, CI |
| 1 | Minimal Streaming Chat | **COMPLETE** | SSE streaming, config, REPL |
| 2 | Tool System | **COMPLETE** | 6 tools, registry, agent loop |
| 3 | Permission System | Not started | |
| 4 | Session Management | Not started | |
| 5 | Rich Terminal UI | Not started | |
| 6 | Multi-Provider API | Not started | |
| 7 | LSP Client | Not started | |
| 8 | MCP Integration | Not started | |
| 9 | Plugin System | Not started | |
| 10 | Subagent System | Not started | |
| 11 | Bash Sandboxing | Not started | |
| 12 | Polish & Distribution | Not started | |

## Completed Tasks

- Phase 0: Cargo workspace with 13 crates, shared types (Message, ContentBlock, Tool trait, error hierarchy), CI pipeline, cargo-deny
- Phase 1: chet-api (SSE streaming client), chet-config (TOML settings, API key), chet-cli (clap args, REPL, print mode, slash commands)
- Phase 2: chet-tools (6 built-in tools: Read, Write, Edit, Bash, Glob, Grep), tool registry, chet-core (agent loop with tool use cycles)

## Test Summary

- 26 tests passing (5 SSE parser, 2 config, 19 tools)
- Zero clippy warnings
- `cargo run --bin chet -- --help` and `--version` working

## Open Blockers

- (none)

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
