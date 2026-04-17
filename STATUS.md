# Chet — Status Tracker

## Current Phase: Phase 11 COMPLETE — v0.3.1 shipped

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
| 11 | Third-Party Providers | **COMPLETE** | AWS Bedrock (11a), Vertex AI (11b), CC-compatible env var layer (11c). Feature-flagged to keep default binary small. |

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

- [x] `chet-bedrock` crate scaffold + `bedrock` feature flag on `chet` binary
- [x] `aws-config` credential resolution + `AWS_REGION`/`AWS_PROFILE` honor
- [x] SigV4 request signing (manual sha2+hmac, not full aws-sigv4 crate)
- [x] AWS EventStream frame parser (prelude + headers + payload + CRC)
- [x] `BedrockProvider` implementing `Provider` trait
- [x] Bedrock error → `ApiError` mapping (throttling, quota, auth)
- [x] Provider selection logic in `main.rs` (`CLAUDE_CODE_USE_BEDROCK`, `CHET_USE_BEDROCK`, `--provider`)
- [x] Model alias resolver honoring `[models]` config section
- [x] (11b) `chet-vertex` crate + Vertex provider (Google ADC auth, SSE reuse)
- [x] (11c) CC-compatible env vars (CLAUDE_CODE_USE_BEDROCK/VERTEX, AWS_REGION, GOOGLE_CLOUD_PROJECT)
- [ ] Integration test: mock Bedrock endpoint + canned EventStream responses (deferred — manual test)
- [ ] Docs: `docs/providers.md` covering auth modes, env var priority (deferred)
- [ ] CI: feature-flag build matrix (deferred — default build sufficient for now)
- [ ] `/provider` slash command for runtime provider inspection (deferred)
- [ ] Interactive `chet setup-bedrock` / `chet setup-vertex` wizards (deferred)

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

- 482 unit tests passing (29 api, 17 config, 16 core/agent+subagent+worktree, 59 tools, 55 permissions, 58 session, 35 types, 153 terminal, 15 cli, 29 mcp, 12 bedrock, 4 vertex)
  - 10 agent integration tests (4 cancellation + 1 multi-tool-use + 1 plan-mode-blocking + 1 subagent-e2e + 1 compaction-state + 1 parallel-failure-isolation + 1 mixed-parallel-sequential)
  - 7 additional ignored tests (worktree: require git + filesystem, run with `--ignored`)
- 6 SSE integration tests (mock SSE pipeline, run with `cargo test -p chet-api --test stream_integration -- --ignored`)
- 4 retry integration tests (TCP test server, run with `cargo test -p chet-api --test retry_integration -- --ignored`)
- 10 agent integration tests in cancellation_integration.rs (4 cancellation + 1 multi-tool-use + 1 plan-mode-blocking + 1 subagent-e2e + 1 compaction-state + 1 parallel-failure-isolation + 1 mixed-parallel-sequential, run with `cargo test -p chet-core --test cancellation_integration -- --ignored`)
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

### Completed (56 items)

<details>
<summary>All completed Post-v1 items (click to expand)</summary>

- ~~**Worktree isolation**~~, ~~**Non-interactive mode optimization**~~, ~~**ConfigChange hook event**~~, ~~**File-not-found path suggestions**~~, ~~**Enhanced permission restriction reasons**~~, ~~**Status line**~~, ~~**Memory management**~~, ~~**`chet agents` CLI command**~~, ~~**MCP reconnect resilience**~~, ~~**Session flush on disconnect**~~, ~~**Auto-memory**~~, ~~**Smarter bash permission prefixes**~~, ~~**Config file corruption prevention**~~, ~~**Tool result disk persistence**~~, ~~**`/copy` command**~~, ~~**`/model` human-readable labels**~~, ~~**HTTP hooks**~~, ~~**Effort levels**~~, ~~**Agent name in terminal title**~~, ~~**`InstructionsLoaded` hook event**~~, ~~**Concise subagent reports**~~, ~~**`/resume` shows most recent prompt**~~, ~~**Skip compaction preamble recap**~~, ~~**Compaction preserves images for cache reuse**~~, ~~**Skip skill re-injection on `/resume`**~~ (N/A), ~~**MCP binary content to disk**~~, ~~**Increased output token limits**~~, ~~**`/effort auto`**~~, ~~**`-n` / `--name` session flag**~~, ~~**`/plan` with description**~~, ~~**Memory file timestamps**~~, ~~**`PostCompact` hook event**~~, ~~**`/context` actionable suggestions**~~, ~~**Parallel tool failure isolation**~~, ~~**Strip progress messages during compaction**~~, ~~**Background bash output kill limit**~~, ~~**Session auto-naming from plan content**~~, ~~**`allowRead` sandbox setting**~~, ~~**`ExitWorktree` tool**~~, ~~**Auto-compaction circuit breaker**~~, ~~**`autoMemoryDirectory` setting**~~, ~~**Token estimation audit**~~, ~~**`StopFailure` hook event**~~, ~~**MCP deny rule enforcement**~~, ~~**Worktree hooks/config loading**~~, ~~**Custom model option**~~, ~~**Agent frontmatter**~~, ~~**MCP elicitation**~~ (N/A — no servers use it), ~~**`--resume` filter print-mode sessions**~~ (deferred), and more.

</details>

### Worth Doing — High Value, Reasonable Effort (15 items)

- **VCS directory exclusions**: Add `.jj` (Jujutsu) and `.sl` (Sapling) to Grep/Glob exclusion lists alongside `.git`. ~5 min.
- **MCP tool description cap**: Cap MCP tool descriptions at 2KB to prevent OpenAPI-generated servers from bloating context window. ~10 min.
- **Token count formatting**: Display >=1M tokens as "1.5m" instead of "1512.6k" in status line and `/context`. ~10 min.
- **Tool result file cleanup**: Clean up `.chet-tool-output/` files after configurable period (`cleanup_period_days`). ~20 min.
- **Stream idle timeout**: Configurable watchdog for hanging SSE streams (default 90s). Kill and surface error instead of hanging indefinitely. ~30 min.
- **`--bare` flag**: Minimal startup for scripted/CI `-p` calls — skip hooks, memory, MCP, plugin sync. Faster cold start. ~20 min.
- **`--resume` filter print-mode sessions**: Don't show `-p` (print mode) sessions in the resume picker. ~10 min.
- **Edit without prior Read**: Allow Edit on files the model has seen via Bash output (cat, sed -n), not just via the Read tool. ~15 min.
- **Read-only glob auto-allow**: `ls *.ts`, `cat src/*.rs` shouldn't prompt. Expand read-only detection. ~10 min.
- **`cd <cwd> &&` prefix auto-allow**: Compound bash commands starting with `cd` into CWD shouldn't prompt. ~10 min.
- **UTF-8 chunk boundary audit**: Audit SSE parser for multi-byte sequences split across HTTP chunks. ~20 min.
- **xhigh effort level for Opus 4.7**: Add `xhigh` effort level (64K tokens). ~5 min.
- **Plan file naming from prompt**: Name plan files after the user's prompt instead of session-id + timestamp. ~10 min.
- **`ENABLE_PROMPT_CACHING_1H` env var**: Opt into 1-hour prompt cache TTL. ~10 min.
- **MCP connection non-blocking in print mode**: Bound MCP connection wait at 5s in `-p` mode. ~15 min.

### Nice to Have — Moderate Value (20 items)

- **Session ID header**: Add `X-Chet-Session-Id` to API requests for proxy aggregation.
- **Conditional hook `if` field**: Filter hooks by tool pattern to reduce process spawning.
- **Background bash stuck notification**: Detect interactive prompt hangs (~45s timeout).
- **Protected directory list**: `.husky`, `.github/workflows` require explicit write permission.
- **Bash stale-edit warning**: Warn when formatter modifies previously-read files.
- **Per-model `/cost` breakdown**: Break down cost by model with cache-hit stats.
- **Prompt cache expiry warning**: Warn about uncached tokens when resuming after TTL.
- **Long Retry-After visibility**: Show countdown instead of silent wait on long backoffs.
- **429 exponential backoff minimum**: Enforce exponential floor even with small Retry-After.
- **Honor `API_TIMEOUT_MS`**: Env var for all HTTP request timeouts.
- **Rich rate-limit error messages**: Show which limit and when it resets.
- **Network errors show retry immediately**: Display "Retrying in Ns" instead of silent spinner.
- **`PreCompact` hook event**: Fire before compaction starts (pairs with PostCompact).
- **Hook errors include stderr first line**: Surface first stderr line for self-diagnosis.
- **Read tool dedup unchanged re-reads**: Hash-based dedup to reduce token usage.
- **Idle-return prompt**: Suggest `/clear` or `/compact` after 75+ min idle.
- **SSE large frame performance audit**: Check for quadratic behavior on large frames.
- **Worktree resume restoration**: `--resume` restores worktree automatically.
- **Print mode partial response preservation**: Preserve partial output on Ctrl+C.
- **Piped compound command denies**: Deny rules must win across all pipe subcommands.

### Skip / Extremely Niche (39 items)

- **LSP Client**: Deferred by design (heavyweight, 1-2GB RAM, low demand).
- **MCP elicitation**: Complex JSON-RPC extension, no servers use it.
- **Rate limit display in status line**: Requires API info we don't have.
- **`CwdChanged`/`FileChanged` hook events**: Over-engineering.
- **`PermissionDenied` hook**: Edge case.
- **Transcript search**: Major UI feature, not CLI-critical.
- **MCP result size override**: Niche MCP extension.
- **Hook `defer` permission decision**: Complex CI/CD pattern.
- **Hook output disk persistence**: Niche.
- **Format-on-save hook conflict**: Very niche.
- **Tool input JSON-encoded field audit**: No known bug.
- **Empty thinking text block audit**: No known bug.
- **Session title via hook**: Niche.
- **`--resume` across worktrees**: Complex, niche.
- **False prompts for `/` in argument values**: Edge case in arg parsing.
- **Backslash-escaped flag security audit**: Security edge case.
- **Env-var prefix allowlist**: Niche security hardening.
- **`/dev/tcp` and `/dev/udp` redirect audit**: Very niche bash security.
- **`grep -f FILE` pattern file access**: Very niche.
- **Stream 5-min stall abort + non-streaming retry**: Complex, rare scenario.
- **`PreToolUse` additionalContext preservation**: Niche hook pattern.
- **Subagent CWD leakage audit**: Audit, no known bug.
- **Subagent worktree Read/Edit access**: Complex edge case.
- **Stale worktree cleanup for squash-merged PRs**: Niche git workflow.
- **Interactive `/effort` slider**: Over-engineering `/effort`.
- **`--resume` defaults to current directory**: Breaking change risk.
- **CLI near-miss typo suggestions**: Nice but complex (Levenshtein).
- **Markdown blockquote continuous left bar**: Visual polish.
- **`Ctrl+U` clears entire input**: Minor readline convention.
- **Cedar policy syntax highlighting**: Way over-engineering.
- **`/model` cache miss warning**: Niche.
- **On-demand syntect grammar loading**: Memory optimization, marginal.
- **OS CA certificate store trust**: Platform-specific, reqwest handles most cases.
- **OpenTelemetry tracing support**: Enterprise feature.
- **`refreshInterval` status line config**: Config knob, low demand.
- **Wildcard rule whitespace normalization**: Edge case.
- **Wildcard rule whitespace normalization**: Edge case in rule matching.
- **`--resume` across worktrees of same repo**: Complex, niche.
- **Long Retry-After visibility**: Already partially handled by retry display.

## Coding Standards

### File Size Convention
Enforced by CI (`file-size` job in `.github/workflows/ci.yml`):

- **Source files**: Max **650 production lines** (lines before `#[cfg(test)]`). Inline unit tests don't count toward the limit.
- **Integration test files** (in `tests/` dirs): Max **800 total lines**.
- **Binary entry points** (`main.rs`): Should be a thin wrapper (~150-250 lines). Delegate to modules.

When a file exceeds the limit, split by single responsibility into submodules. Re-export the public API from the parent module so callers don't change.

### Module Organization
- `chet-cli/src/`: `main.rs` (entry), `repl.rs`, `commands.rs`, `runner.rs`, `context.rs` (parameter structs), `plan.rs`, `prompts.rs`, `prompt.rs`
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
| 2026-04-17 | v0.3.1 refactoring true-up | Decomposed Agent::run() (440→100 lines), introduced CLI context structs (10-11 params→2-3), centralized 4 workspace deps, added Spinner Drop safety net, converted API retry body to Bytes, fixed grep case_insensitive/context bugs, added 24 tool error-path tests (35→59 in chet-tools) |
