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
| 11 | Third-Party Providers | **PLANNED** | AWS Bedrock (11a), Vertex AI (11b), CC-compatible env var layer (11c). Feature-flagged to keep default binary small. |

### Phase 11 Plan

Add AWS Bedrock and Google Vertex AI support, building on the existing `Provider` trait (Phase 7.5). Feature-flagged (`bedrock`, `vertex`) so non-cloud users pay zero cost.

**11a: Bedrock Provider** — `chet-bedrock` crate implementing `Provider`
- Auth: `aws-config` + `aws-credential-types` + `aws-sigv4` crates (pure Rust, ~15–20 transitive deps). Handles the full AWS credential chain: env vars, `AWS_PROFILE`, SSO, assumed roles (`source_profile` + STS), IMDS. Required because real-world profiles like `claude-pr-reviewer` are typically assumed-role chains.
- HTTP: reuse `reqwest` + our SSE infrastructure. Bedrock endpoint: `https://bedrock-runtime.<region>.amazonaws.com/model/<model-id>/invoke-with-response-stream`
- Wire format: AWS EventStream frame parser (~100-150 LOC) → unwraps `chunk` events → payloads are standard Anthropic `StreamEvent` JSON (same format used by `chet-api` today). Reference: `aws-smithy-eventstream` crate if helpful.
- Error mapping: `ThrottlingException` → `ApiError::RateLimited`, `ServiceQuotaExceededException` → `Overloaded`, etc. Retry/backoff works unchanged.
- Credential refresh: hold `aws-config` provider across requests, re-resolve per request (cheap, cached internally). Don't snapshot creds at startup.
- Config: `[providers.bedrock]` in `config.toml` with `region` (optional — defaults to `AWS_REGION` env). No direct credential fields; rely on AWS SDK chain.

**11b: Vertex AI Provider** — `chet-vertex` crate, follow-on
- Auth: GCP ADC (Application Default Credentials) via `gcp_auth` crate or similar
- Endpoint: `https://<region>-aiplatform.googleapis.com/v1/projects/<project>/locations/<region>/publishers/anthropic/models/<model>:streamRawPredict`
- Wire format: SSE (same as Anthropic direct), with a thin GCP request envelope
- Config: `[providers.vertex]` with `project_id`, `region`, optional credential path

**11c: CC-compatible env var layer** — shared infrastructure
- Provider selection: honor `CLAUDE_CODE_USE_BEDROCK=1` / `CLAUDE_CODE_USE_VERTEX=1` alongside our native `CHET_USE_BEDROCK=1` and `--provider bedrock` CLI flag
- Model alias resolution priority: CLI `--model` → `ANTHROPIC_MODEL` env → `[models]` config → hard default
- Alias env vars: `ANTHROPIC_DEFAULT_SONNET_MODEL`, `_HAIKU_MODEL`, `_OPUS_MODEL` override short names (`sonnet` / `haiku` / `opus`) when resolving. Means `chet --model sonnet` transparently resolves to a Bedrock inference profile ID when these vars are set.
- Drop-in compatibility: a user whose shell already exports the CC Bedrock vars should be able to run `chet` with zero additional config.

### Phase 11 Checklist

- [ ] `chet-bedrock` crate scaffold + `bedrock` feature flag on `chet` binary
- [ ] `aws-config` credential resolution + `AWS_REGION`/`AWS_PROFILE` honor
- [ ] SigV4 request signing via `aws-sigv4` (wraps `reqwest::Request`)
- [ ] AWS EventStream frame parser (prelude + headers + payload + CRC32)
- [ ] `BedrockProvider` implementing `Provider` trait
- [ ] Bedrock error → `ApiError` mapping (throttling, quota, auth)
- [ ] Provider selection logic in `main.rs` (`CLAUDE_CODE_USE_BEDROCK`, `CHET_USE_BEDROCK`, `--provider`)
- [ ] Model alias resolver honoring `ANTHROPIC_MODEL` + `ANTHROPIC_DEFAULT_{SONNET,HAIKU,OPUS}_MODEL`
- [ ] Integration test: mock Bedrock endpoint + canned EventStream responses
- [ ] Docs: `docs/providers.md` covering Bedrock auth modes, env var priority, model ID formats
- [ ] CI: feature-flag build matrix (default, `--features bedrock`)
- [ ] (11b) `chet-vertex` crate + Vertex provider
- [ ] (11b) Vertex integration test
- [ ] (11c) `/provider` slash command for runtime provider inspection
- [ ] Interactive `chet setup-bedrock` / `chet setup-vertex` wizards — guide user through credentials, region, project, and model pinning. Seed from existing pins on re-run. Offer "with 1M context" option for supported models. (CC v2.1.98, v2.1.111)

### Phase 11 Gotchas (learned from Claude Code fixes)

- **Don't set `Authorization` header when using SigV4** — `ANTHROPIC_AUTH_TOKEN`, `apiKeyHelper`, `ANTHROPIC_CUSTOM_HEADERS` setting Authorization will break SigV4 signing with 403. Strip these on Bedrock requests. (CC v2.1.101)
- **Empty `AWS_BEARER_TOKEN_BEDROCK` = not set** — GitHub Actions exports empty strings for unset inputs. Treat empty-string env vars as absent before deciding bearer vs SigV4 auth. (CC v2.1.97)
- **Model picker for non-US Bedrock regions** — Don't persist `us.*` inference profile IDs to config when the user is in a non-US region. Wait for inference profile discovery to complete before writing. (CC v2.1.105)
- **Rate limits don't reference status.claude.com on Bedrock/Vertex** — That page only covers Anthropic-operated providers. Use cloud-provider-specific status links or omit. (CC v2.1.111)

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

- 438 unit tests passing (34 api, 17 config, 16 core/agent+subagent+worktree, 35 tools, 52 permissions, 58 session, 27 types, 153 terminal, 15 cli, 29 mcp)
  - 10 agent integration tests (4 cancellation + 1 multi-tool-use + 1 plan-mode-blocking + 1 subagent-e2e + 1 compaction-state + 1 parallel-failure-isolation + 1 mixed-parallel-sequential)
  - 7 additional ignored tests (worktree: require git + filesystem, run with `--ignored`)
- 6 SSE integration tests (mock SSE pipeline, run with `cargo test -p chet-api --test stream_integration -- --ignored`)
- 4 retry integration tests (TCP test server, run with `cargo test -p chet-api --test retry_integration -- --ignored`)
- 8 agent integration tests in cancellation_integration.rs (4 cancellation + 1 multi-tool-use + 1 plan-mode-blocking + 1 subagent-e2e + 1 compaction-state, run with `cargo test -p chet-core --test cancellation_integration -- --ignored`)
- 1 pipe mode integration test (ANSI-free output, run with `cargo test -p chet --test pipe_mode -- --ignored`)
- 3 MCP end-to-end tests (Python MCP server, run with `cargo test -p chet-mcp --test mcp_e2e -- --ignored`)
- 3 session round-trip tests (filesystem, run with `cargo test -p chet-session --test session_roundtrip -- --ignored`)
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
- ~~**Subagent end-to-end**~~: **DONE** — `test_subagent_end_to_end` in cancellation_integration.rs. Parent agent calls SubagentTool via SequencedMockProvider (3 calls: parent tool_use → child text → parent final text). Validates full SubagentTool → child Agent → text result → parent pipeline.
- ~~**Retry/backoff**~~: **DONE** — 4 `#[ignore]` tests in `crates/chet-api/tests/retry_integration.rs`. Raw TCP test server (no new deps) returns 429/500/401 responses. Covers: 429→retry→success, 500→retry→success, retry exhaustion, and non-retryable 401 (no retry). Run with `cargo test -p chet-api --test retry_integration -- --ignored`.
- ~~**Multi-tool-use turn**~~: **DONE** — `test_multi_tool_use_turn` in cancellation_integration.rs. SequencedMockProvider returns 2 tool_use blocks, verifies both execute and results sent back, validates message structure (4 messages: user → assistant(2 tool_use) → user(2 tool_result) → assistant(text)).
- ~~**Plan mode tool blocking**~~: **DONE** — `test_plan_mode_tool_blocking` in cancellation_integration.rs. Agent in read-only mode, SequencedMockProvider requests non-read-only tool. Verifies ToolBlocked event fires, tool not executed, error ToolResult sent back, agent continues to final text response.
- ~~**Non-interactive pipe mode**~~: **DONE** — `test_pipe_mode_no_ansi` in `crates/chet-cli/tests/pipe_mode.rs`. SequencedMockProvider drives tool-use turn + markdown text response through full event callback pipeline with TTY=false. Captures stdout (plain markdown renderer) and stderr (style functions), asserts zero `\x1b` bytes. Run with `cargo test -p chet --test pipe_mode -- --ignored`.
- ~~**Session round-trip**~~: **DONE** — 3 `#[ignore]` tests in `crates/chet-session/tests/session_roundtrip.rs`. Realistic 8-message session with Text, ToolUse, ToolResult content blocks, usage, metadata label, compaction count. Covers: complex round-trip, list/summary, and save-modify-save update. Run with `cargo test -p chet-session --test session_roundtrip -- --ignored`.
- ~~**MCP end-to-end**~~: **DONE** — 3 `#[ignore]` tests in `crates/chet-mcp/tests/mcp_e2e.rs`. Inline Python script as minimal MCP server. Covers: connect + tool discovery, call_tool with echo result, unknown tool error. Run with `cargo test -p chet-mcp --test mcp_e2e -- --ignored`.
- ~~**Compaction state preservation**~~: **DONE** — `test_compaction_preserves_label_and_plan_mode` in cancellation_integration.rs. Builds 14-message conversation, compacts with label, verifies label in summary, feeds compacted messages into read-only agent, verifies writable tool still blocked. Run with `cargo test -p chet-core --test cancellation_integration -- --ignored`.

## Product Direction

- **CI/CD-first agent**: Leverage compiled Rust binary for zero-dependency, fast-startup headless agent mode targeting CI/CD pipelines (PR review, code checks, test generation). Key differentiator vs Node.js-based tools.

## Post-v1

- **LSP Client**: Opt-in (default off), --lsp flag or [lsp] config. Grep/Read cover 90% of needs; LSP is heavyweight (1-2GB RAM) and hurts CI/CD. Revisit if users request it. Filter gitignored files from results.
- ~~**Worktree isolation**~~: **DONE** — `--worktree` flag + subagent `isolation: "worktree"` for parallel agents in isolated git worktrees. `WorktreeCreate`/`WorktreeRemove` hook events, RAII cleanup via `ManagedWorktree`.
- ~~**Non-interactive mode optimization**~~: **DONE** — TTY detection via `std::io::IsTerminal`, plain markdown passthrough, silent spinner, plain tool events, no ANSI in piped output.
- ~~**ConfigChange hook event**~~: **DONE** — Background task polls `~/.chet/config.toml` and `.chet/config.toml` mtimes every 5s. On change, fires `config_change` hook with `config_path` in payload. No new deps (uses std mtime instead of notify crate). 2 new tests.
- ~~**File-not-found path suggestions**~~: **DONE** — Read/Edit tools now walk the repo (via .git discovery) on NotFound and suggest files with matching basenames. Skips target/, node_modules/, .venv/, etc. Cap 5 suggestions, max depth 8. New `path_suggest` module with 6 tests.
- ~~**Enhanced permission restriction reasons**~~: **DONE** — Block/Prompt messages now include the matched input args: "Tool 'Bash' (command: rm -rf /tmp/foo) blocked by permission (rule: Bash [command:rm *] -> block)". Extracts key string fields (command, file_path, path, url, pattern), truncated at 80 chars. 3 new tests.
- ~~**Status line**~~: **DONE** — Persistent terminal status bar (DECSTBM scroll region) showing model, context usage, tokens, effort, session ID, plan mode badge, and active tool. Updates in real-time during agent execution. Suspend/resume around line editor. SIGWINCH resize handling. TTY-only (skipped in print mode).
- ~~**Memory management**~~: **DONE** — Audit complete. Session rules deduplicated (prevents unbounded "always allow" growth). SSE pending_events switched from Vec to VecDeque (O(1) pop_front). History already capped at 1000. MCP/tool registries static after init. Messages bounded by compaction. No unbounded caches found.
- ~~**`chet agents` CLI command**~~: **DONE** — `chet agents` subcommand lists configured `[agents.<name>]` profiles with their effort, max_turns, disallowed_tools, and system_prompt preview. Uses clap Subcommand pattern.
- ~~**MCP reconnect resilience**~~: **DONE** — `/mcp reconnect [name]` shuts down and reconnects specified (or all) MCP servers. Unknown names show available servers instead of freezing. McpManager stores config for reconnection.
- ~~**Session flush on disconnect**~~: **DONE** — SIGHUP handler (unix) cancels the current agent turn via CancellationToken, causing clean return to REPL which auto-saves the session. Same pattern as Ctrl+C.
- ~~**Auto-memory**~~: **DONE** — MemoryRead/MemoryWrite tools + `/memory` command. Global (`~/.chet/memory/MEMORY.md`) and per-project (`~/.chet/memory/projects/<hash>.md`) scopes. Loaded into system prompt, refreshed after each turn. Atomic writes, $EDITOR support, worktree-safe (hashes original cwd).
- ~~**Smarter bash permission prefixes**~~: **DONE** — Compound commands split on `&&`, `||`, `;`, `|` (quote-aware) for per-subcommand rule matching. `command:rm *` now catches `cd /tmp && rm -rf /`. 11 new tests.
- ~~**Config file corruption prevention**~~: **DONE** — All file writes now use atomic tmp+rename: history, Write tool, Edit tool, plan files, compaction archives (sessions and memory already had it). `atomic_write_file` utility in chet-types. 3 new tests.
- ~~**Tool result disk persistence**~~: **DONE** — Tool results >50K chars persisted to `.chet-tool-output/<tool>-<id>.txt` under CWD, truncated in context with path reference. Model can re-read via Read tool if needed.
- ~~**`/copy` command**~~: **DONE** — Copies last assistant response to system clipboard (pbcopy/xclip/xsel/clip). Falls back to printing to stdout if clipboard unavailable.
- ~~**`/model` human-readable labels**~~: **DONE** — `/model` shows "sonnet-4.5 (claude-sonnet-4-5-20250929)". `/sessions` list also uses short names. Reuses existing `shorten_model_name`.
- ~~**HTTP hooks**~~: **DONE** — Hook commands starting with `http://` or `https://` POST the JSON payload to the URL. Response protocol: 2xx=approve, 403=deny, other=error. Uses reqwest with configurable timeout.
- ~~**Effort levels**~~: **DONE** — `--effort` CLI flag (low/medium/high) maps to thinking budget_tokens (1024/8192/32768). `/effort` REPL command for per-turn changes. Effort shown in spinner and startup banner. Explicit `--thinking-budget` takes precedence.
- ~~**Agent name in terminal title**~~: **DONE** — Terminal title set to "chet — <session-id>" on start, updated to "chet — <label>" when auto-label fires, reset on exit. OSC escape sequence, TTY-only.
- ~~**`InstructionsLoaded` hook event**~~: **DONE** — `instructions_loaded` hook fires after system prompt (with memory) is set at session start. Enables validation hooks on loaded instructions.
- ~~**Concise subagent reports**~~: **DONE** — Subagent results truncated at 10K chars with "(subagent output truncated from N chars)" marker. Keeps parent context lean.
- ~~**`/resume` shows most recent prompt**~~: **DONE** — `preview()` now returns the last user text message instead of the first. `/sessions` list shows the most recent prompt for each session. 1 new test.
- ~~**Skip compaction preamble recap**~~: **DONE** — Compaction summary shortened from verbose "[This conversation was compacted...]" to terse "[Compacted conversation summary:]". Saves ~15 tokens per compaction.
- ~~**Compaction preserves images for cache reuse**~~: **DONE** — Already handled: `strip_heavy_payloads` only drops Thinking blocks; Image blocks pass through `other => Some(other.clone())`.
- ~~**Skip skill re-injection on `/resume`**~~: **N/A** — Chet has no skill injection system. No action needed.
- ~~**MCP binary content to disk**~~: **DONE** — MCP image/binary content decoded from base64, saved to `.chet-mcp-output/mcp-<uuid>.{ext}` with correct extension (png/jpg/pdf/docx/xlsx/mp3/etc). Text reference returned to context instead of raw base64.
- ~~**Increased output token limits**~~: **DONE** — Default max_tokens bumped from 16k to 64k.
- ~~**`/effort auto`**~~: **DONE** — `/effort auto` resets effort to default (no explicit thinking budget). Help text and error messages updated.
- ~~**`-n` / `--name` session flag**~~: **DONE** — `chet -n "my task"` sets session label at startup, overrides auto-label. Works with `--resume` too.
- ~~**`/plan` with description**~~: **DONE** — `/plan fix the auth bug` enters plan mode and immediately sends the description as a message. Already in plan mode? Just sends the message.
- ~~**Memory file timestamps**~~: **DONE** — Memory section headings include "(last updated: YYYY-MM-DD HH:MM UTC)" from file mtime. Model can reason about freshness.
- ~~**`PostCompact` hook event**~~: **DONE** — `post_compact` hook fires after `/compact` with `messages_removed` and `messages_remaining` in the JSON payload.
- ~~**`/context` actionable suggestions**~~: **DONE** — `/context` now suggests `/compact` at >50%/>80% usage and warns about large system prompts (>20% of context window) with `/memory reset` hint.
- ~~**Parallel tool failure isolation**~~: **DONE** — Read-only tools (Read, Glob, Grep, MemoryRead) now execute in parallel via `join_all`. Mutating tools run sequentially after. Failures produce per-tool error results without affecting siblings. Permission checks and hooks remain sequential (may prompt user).
- ~~**Strip progress messages during compaction**~~: **DONE** — Recent messages preserved after compaction now have large ToolResult text truncated (>4000 chars) and Thinking blocks removed. Prevents context bloat from file reads, grep outputs, and bash outputs surviving compaction. 4 new tests.
- ~~**Background bash output kill limit**~~: **DONE** — Bash tool now spawns child with piped stdout/stderr, reads via `bounded_read` (cap per stream), and kills the process if total output exceeds 5GB. Prevents runaway processes from exhausting memory.
- ~~**Session auto-naming from plan content**~~: **DONE** — When user approves a plan, the first heading (or first line) of the plan text becomes the session label (if not already named). `label_from_plan()` strips `#` prefixes, truncates to 60 chars. 4 new tests.
- **MCP elicitation**: MCP servers can request structured input mid-task via interactive dialog (form fields). New JSON-RPC protocol extension.
- **`allowRead` sandbox setting**: Re-allow read access within `denyRead` regions for fine-grained sandbox control.
- **`ExitWorktree` tool**: Allow leaving a worktree session from within (counterpart to `EnterWorktree`).
- **Auto-compaction circuit breaker**: If auto-compaction is added, stop retrying after 3 consecutive failures.
- ~~**`autoMemoryDirectory` setting**~~: **DONE** — `memory_dir` in config.toml overrides default `~/.chet/memory/`. Resolved in `ChetConfig`, flows through `MemoryManager` and both memory tools.
- ~~**Token estimation audit**~~: **DONE** — Thinking blocks excluded (not in input context). Text uses chars/3.5 (was chars/4). JSON/tool inputs use chars/5. ToolUse/ToolResult add fixed overhead for IDs. 4 new tests.
- ~~**`StopFailure` hook event**~~: **DONE** — `stop_failure` hook fires on API errors (stream error or network failure) with error message in `tool_output` field. Best-effort, log-only.
- ~~**MCP deny rule enforcement**~~: **DONE** — `is_tool_blocked()` on PermissionEngine checks static block rules. Tool definitions filtered via `defs.retain()` before sending to API — model never sees blocked tools. 2 new tests.
- ~~**Worktree hooks/config loading**~~: **DONE** — `load_with_project_dir()` loads `.chet/config.toml` from the project directory and merges hooks + permission rules with global config. Called with CWD on startup.
- ~~**Custom model option**~~: **DONE** — `[models]` config section for aliases (e.g. `fast = "claude-haiku-4-5-20251001"`). Aliases resolved during config load. 2 new tests.
- ~~**Agent frontmatter**~~: **DONE** — `[agents.<name>]` config section with `effort`, `max_turns`, `disallowed_tools`, `system_prompt` fields. `AgentConfig` struct in chet-config. 2 new tests.
- **VCS directory exclusions**: Add `.jj` (Jujutsu) and `.sl` (Sapling) to Grep/Glob exclusion lists alongside `.git`.
- **MCP tool description cap**: Cap MCP tool descriptions at 2KB to prevent OpenAPI-generated servers from bloating context window.
- **Token count formatting**: Display >=1M tokens as "1.5m" instead of "1512.6k" in status line and `/context`.
- **Tool result file cleanup**: Clean up `.chet-tool-output/` files after configurable period (`cleanup_period_days`). Persistence exists but no housekeeping.
- **Session ID header**: Add `X-Chet-Session-Id` header to API requests for proxy aggregation and debugging.
- **Stream idle timeout**: Configurable watchdog for hanging SSE streams (default 90s). Kill and surface error instead of hanging indefinitely.
- **Conditional hook `if` field**: Filter hooks by tool pattern (e.g., `Bash(git *)`). Uses permission rule syntax. Reduces unnecessary process spawning.
- **Read tool dedup unchanged re-reads**: Track recently-read file content hashes, skip re-sending unchanged files to reduce token usage.
- **`--bare` flag**: Minimal startup for scripted/CI `-p` calls — skip hooks, memory, MCP, plugin sync. Faster cold start.
- **Idle-return prompt**: Suggest `/clear` or `/compact` after 75+ minutes idle to avoid stale context and token waste. Hint should show *current* context size, not cumulative session tokens.
- **Background bash stuck notification**: Surface notification when bash appears stuck on an interactive prompt (~45s timeout).
- **Rate limit display in status line**: Show API rate limit usage percentages and reset time.
- **`CwdChanged`/`FileChanged` hook events**: Reactive hooks for environment management (e.g., direnv-style auto-reload).
- **`PermissionDenied` hook**: Fire hook after permission denials. Return `{retry: true}` to let model retry.
- **Transcript search**: Search through conversation history (`/` in transcript mode, `n`/`N` to step through matches).
- **Worktree resume restoration**: `--resume` on a session that was in a worktree should restore that worktree automatically.
- **SSE large frame performance audit**: Audit SSE parser for quadratic behavior on large streamed frames (CC v2.1.90 found and fixed this).
- **MCP result size override**: Allow MCP servers to specify max result size via `_meta["maxResultSizeChars"]` annotation (up to 500K), preventing truncation of large but valuable results like DB schemas.
- **`--resume` filter print-mode sessions**: Don't show `-p` (print mode) sessions in the resume picker. They're one-shot and not useful to resume interactively.
- **Protected directory list**: Add `.husky`, `.github/workflows`, and other CI/config dirs to directories requiring explicit write permission.
- **Hook `defer` permission decision**: PreToolUse hooks can return `"defer"` to pause execution. Headless sessions resume with `-p --resume` for later re-evaluation. CI/CD pattern.
- **MCP connection non-blocking in print mode**: Skip MCP server connection wait in `-p` mode, bound connection time at 5s instead of blocking on slowest server.
- **Edit without prior Read**: Allow Edit on files the model has seen via Bash output (cat, sed -n), not just via the Read tool.
- **Hook output disk persistence**: Hook output >50K chars saved to disk with file path + preview instead of injecting directly into context.
- **Bash stale-edit warning**: Warn when a formatter/linter modifies previously-read files, preventing stale-edit errors on subsequent Edit calls.
- **Format-on-save hook conflict**: Handle PostToolUse hooks that rewrite files between consecutive Edit/Write calls (e.g., rustfmt). Detect changed content and re-read before next edit.
- **Per-model `/cost` breakdown**: Show `/cost` broken down by model with cache-hit stats (cache_creation / cache_read / cache_miss tokens). Current implementation shows totals only.
- **Prompt cache expiry warning**: When resuming a session after the prompt cache has likely expired (~5 min TTL), show a hint about how many tokens the next turn will send uncached. Helps users understand cost surprises.
- **Tool input JSON-encoded field audit**: Audit tool input parsing for array/object fields arriving as JSON-encoded strings during streaming (CC v2.1.92 fix). May affect our streaming tool input handling.
- **Empty thinking text block audit**: Audit extended thinking handling for whitespace-only text blocks appearing alongside real content. Can trigger API 400s on next turn.
- **Session title via hook**: `UserPromptSubmit` hook (or similar) can return `sessionTitle` in `hookSpecificOutput` JSON to set/override the session label. Integrates with auto-label system.
- **Long Retry-After visibility**: When server returns a long `Retry-After` header, surface it immediately with a countdown or error instead of silently waiting. Prevents apparent hang on long backoff periods.
- **`--resume` across worktrees of same repo**: Resume sessions from other worktrees of the same underlying repo directly, not just the current worktree/cwd. Currently `--resume` is bound to the current directory context.
- **Print mode partial response preservation**: If `chet -p` is interrupted mid-stream (Ctrl+C, SIGHUP), preserve whatever assistant text has arrived so far in output/session history instead of discarding the partial response.
- **UTF-8 chunk boundary audit**: Audit SSE parser for multi-byte UTF-8 sequences split across HTTP chunk boundaries. Confirm we buffer raw bytes and don't decode to string before splitting on `\n\n`.

### Bash Permission Hardening (from CC v2.1.97–v2.1.98 fixes)

- **Read-only glob auto-allow**: `ls *.ts`, `cat src/*.rs` shouldn't prompt. Expand read-only detection to handle glob patterns in argument lists.
- **`cd <cwd> &&` prefix auto-allow**: Compound bash commands starting with `cd` into the current project directory shouldn't prompt. Common model pattern — always prompting is noise.
- **Wildcard rule whitespace normalization**: `Bash(git commit *)` rules currently fail to match commands with extra spaces or tabs. Normalize whitespace before matching.
- **Piped compound command denies**: `Bash(...)` deny rules get downgraded to prompt for piped commands that mix `cd` with other segments. Deny must win across all subcommands.
- **False prompts for `/` in argument values**: `cut -d /`, `paste -d /`, `column -s /`, `awk '{print $1}' file`, filenames with `%`. Argument parsing treats these as path-like when they're delimiters/format specifiers.
- **Backslash-escaped flag security audit**: CC had a bypass where `\-rm` could auto-allow as read-only then execute destructively. Audit our bash parser for equivalent escape handling.
- **Env-var prefix allowlist**: `FOO=bar cmd` should only auto-allow when `FOO` is a known-safe var (`LANG`, `TZ`, `NO_COLOR`, `LC_*`, etc.). Arbitrary env vars can change command behavior.
- **`/dev/tcp/...` and `/dev/udp/...` redirect audit**: Bash pseudo-device redirects enable network I/O. Must prompt, never auto-allow.
- **`grep -f FILE` / `rg -f FILE` pattern file access**: External pattern file is read — must check Read permission against that file's path.

### API / Retry / Streaming (from CC v2.1.97–v2.1.108 fixes)

- **`ENABLE_PROMPT_CACHING_1H` env var**: Opt into 1-hour prompt cache TTL (default is 5 min). Add to cache control headers on system prompt + last tool definition.
- **Stream 5-min stall abort + non-streaming retry**: Extends existing stream idle timeout roadmap item. After 5 min with no data, abort the SSE stream and retry as non-streaming request.
- **429 exponential backoff minimum**: Even when `Retry-After` is small, enforce exponential backoff as a floor. Prevents burning all retry attempts in ~13s on aggressive rate-limit advice.
- **Honor `API_TIMEOUT_MS`**: Audit for hardcoded request timeouts (CC had a 5-min cap). `API_TIMEOUT_MS` env var should govern all HTTP request timeouts, including for slow backends / extended thinking.
- **Rich rate-limit error messages**: Show which limit was hit (server throttle vs plan usage) and when it resets, instead of an opaque seconds countdown.
- **Network errors show retry immediately**: When connection fails, display "Retrying in Ns" immediately instead of a silent spinner until backoff expires.

### Hooks

- **`PreCompact` hook event**: Fire hook before compaction starts. Hook can block with exit code 2 or `{"decision":"block"}` JSON. Pairs with existing `PostCompact`.
- **Hook errors include stderr first line**: When a hook fails, surface the first line of stderr in the transcript so users can self-diagnose without `--debug`.
- **`PreToolUse` `additionalContext` preserved on tool failure**: When a tool call fails, don't drop `additionalContext` the hook provided. Currently gets discarded alongside the failed tool result.

### Subagent / Worktree

- **Subagent worktree isolation CWD leakage audit**: When a subagent runs with worktree isolation, its `cwd:` override or resolved worktree path should not leak back to the parent session's Bash tool context. CC had this bug; audit our impl.
- **Subagent worktree Read/Edit access**: Subagents in isolated worktrees should always have Read/Edit access to files inside their own worktree directory, regardless of parent permission config.
- **Stale worktree cleanup for squash-merged PRs**: Current cleanup keeps worktrees indefinitely for subagent PRs that were squash-merged. Detect squash-merged branches (no shared tip with main but logically merged) and clean up.

### Terminal / UX

- **Interactive `/effort` slider**: When `/effort` is called without args, open an interactive picker. Arrow keys navigate low/medium/high/xhigh/auto, Enter to confirm.
- **`--resume` defaults to current directory**: Resume picker shows only current-directory sessions by default; Ctrl+A reveals all projects. Reduces noise for users with many repos.
- **CLI near-miss typo suggestions**: `chet udpate` → "Did you mean `chet update`?". Use Levenshtein or similar for subcommand suggestion.
- **Markdown blockquote continuous left bar**: Renderer currently breaks the left bar on wrapped lines. Maintain it across wraps for readability.
- **`Ctrl+U` clears entire input**: Change from "delete to start of line" to "clear entire input buffer" (readline convention shift, matches CC).
- **Cedar policy syntax highlighting**: Add `.cedar` / `.cedarpolicy` syntax to syntect bundled grammars.
- **`/model` cache miss warning**: Warn before switching models mid-conversation — the next response re-reads the full history uncached. Large cost surprise otherwise.

### Memory / Performance

- **On-demand syntect grammar loading**: `CodeHighlighter` currently loads all grammars at init. Load per-language grammars on first use instead. Reduces startup memory.
- **xhigh effort level for Opus 4.7**: Add `xhigh` level between `high` and `max`. Available on Opus 4.7+; other models fall back to `high`. Extend `--effort`, `/effort`, and status line display.

### Other

- **OS CA certificate store trust**: For enterprise TLS proxies (corporate MITM with private root CAs). Add `rustls-native-certs` behind a feature flag or default-on with `CHET_CERT_STORE=bundled` opt-out.
- **OpenTelemetry tracing support**: `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_LOG_TOOL_DETAILS`, `OTEL_LOG_TOOL_CONTENT`, `OTEL_LOG_USER_PROMPTS` env var gating. Emit spans for agent turns, tool calls, API requests. Propagate `TRACEPARENT`/`TRACESTATE` into Bash tool subprocesses for distributed trace linking.
- **`refreshInterval` status line config**: Re-run the status line command every N seconds, not just on state change. Enables live external data in status (git branch, tmux pane, etc).
- **Plan file naming from prompt**: Name plan files after the user's prompt (e.g. `fix-auth-race-snug-otter.md`) instead of short-session-id + timestamp. More scannable `~/.chet/plans/` listing.

## Coding Standards

### File Size Convention
Enforced by CI (`file-size` job in `.github/workflows/ci.yml`):

- **Source files**: Max **650 production lines** (lines before `#[cfg(test)]`). Inline unit tests don't count toward the limit.
- **Integration test files** (in `tests/` dirs): Max **800 total lines**.
- **Binary entry points** (`main.rs`): Should be a thin wrapper (~150-250 lines). Delegate to modules.

When a file exceeds the limit, split by single responsibility into submodules. Re-export the public API from the parent module so callers don't change.

### Module Organization
- `chet-cli/src/`: `main.rs` (entry), `repl.rs`, `commands.rs`, `runner.rs`, `plan.rs`, `prompts.rs`, `prompt.rs`
- `chet-terminal/src/`: `markdown.rs` (renderer), `inline.rs` (inline formatting), `table.rs` (table rendering)
- `chet-core/tests/`: `common/mod.rs` (shared test harness), `cancellation_integration.rs`
- Run `cargo fmt --all` before committing. CI checks formatting.

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
| 2026-04-08 | Phase 11: Bedrock/Vertex providers feature-flagged | `aws-config` + `aws-sigv4` (not full `aws-sdk-bedrockruntime`) for mid-weight deps. Feature flags keep default binary small; CI/CD users unaffected |
| 2026-04-08 | Honor CC env vars (`CLAUDE_CODE_USE_BEDROCK`, `ANTHROPIC_*_MODEL`) | Drop-in compatibility with existing CC user setups. Aliases `sonnet`/`haiku`/`opus` resolve via env vars when set, else `[models]` config, else hard default |
