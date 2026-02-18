# Anthropic API Reference

How Chet interacts with the Anthropic Messages API.

## Endpoint

```
POST {base_url}/v1/messages
```

Default base URL: `https://api.anthropic.com`

## Headers

| Header | Value |
|--------|-------|
| `Content-Type` | `application/json` |
| `x-api-key` | Your Anthropic API key |
| `anthropic-version` | `2023-06-01` |

No beta headers are sent.

## Request Body

Every request uses streaming (`"stream": true`). Here is the full `CreateMessageRequest` structure:

```json
{
  "model": "claude-sonnet-4-5-20250929",
  "max_tokens": 16384,
  "stream": true,
  "messages": [ ... ],
  "system": [ ... ],
  "tools": [ ... ],
  "temperature": null,
  "thinking": null,
  "stop_sequences": null
}
```

Optional fields (`system`, `tools`, `temperature`, `thinking`, `stop_sequences`) are omitted from the JSON when not set.

### Messages

```json
{
  "role": "user",
  "content": [
    { "type": "text", "text": "Hello" }
  ]
}
```

Content block types sent to the API:

| Type | Fields | When used |
|------|--------|-----------|
| `text` | `text` | User prompts, assistant responses |
| `tool_use` | `id`, `name`, `input` | Assistant requesting a tool call |
| `tool_result` | `tool_use_id`, `content`, `is_error?` | Returning tool output to the API |
| `thinking` | `thinking`, `signature?` | Extended thinking (echo back to API) |
| `image` | `source: {type, media_type, data}` | Image content |

### System Prompt

Sent as structured content with prompt caching enabled:

```json
{
  "system": [
    {
      "type": "text",
      "text": "You are Chet, an AI coding assistant...\n\nCurrent working directory: /path/to/cwd",
      "cache_control": { "type": "ephemeral" }
    }
  ]
}
```

### Tool Definitions

All 6 built-in tools are sent on every request. The **last** tool definition gets a `cache_control` marker for prompt caching.

#### Read (read-only)

```json
{
  "name": "Read",
  "description": "Read a file from the filesystem. Returns content with line numbers. Supports offset and limit for reading portions of large files.",
  "input_schema": {
    "type": "object",
    "required": ["file_path"],
    "properties": {
      "file_path": { "type": "string", "description": "Absolute path to the file to read" },
      "offset":    { "type": "integer", "description": "Line number to start reading from (1-based)" },
      "limit":     { "type": "integer", "description": "Maximum number of lines to read" }
    }
  }
}
```

#### Write (mutating)

```json
{
  "name": "Write",
  "description": "Write content to a file. Creates the file and any parent directories if they don't exist. Overwrites existing files.",
  "input_schema": {
    "type": "object",
    "required": ["file_path", "content"],
    "properties": {
      "file_path": { "type": "string", "description": "Absolute path to the file to write" },
      "content":   { "type": "string", "description": "The content to write to the file" }
    }
  }
}
```

#### Edit (mutating)

```json
{
  "name": "Edit",
  "description": "Perform exact string replacements in files. The old_string must be unique in the file unless replace_all is true. Use this for targeted edits to existing files.",
  "input_schema": {
    "type": "object",
    "required": ["file_path", "old_string", "new_string"],
    "properties": {
      "file_path":   { "type": "string",  "description": "Absolute path to the file to edit" },
      "old_string":  { "type": "string",  "description": "The exact text to find and replace" },
      "new_string":  { "type": "string",  "description": "The replacement text" },
      "replace_all": { "type": "boolean", "description": "Replace all occurrences (default: false, requires unique match)" }
    }
  }
}
```

#### Bash (mutating)

```json
{
  "name": "Bash",
  "description": "Execute a bash command. The working directory persists between calls. Output is truncated at 30K characters. Commands time out after 2 minutes by default.",
  "input_schema": {
    "type": "object",
    "required": ["command"],
    "properties": {
      "command": { "type": "string",  "description": "The bash command to execute" },
      "timeout": { "type": "integer", "description": "Timeout in milliseconds (max 600000)" }
    }
  }
}
```

#### Glob (read-only)

```json
{
  "name": "Glob",
  "description": "Find files matching a glob pattern. Results are sorted by modification time (newest first). Respects .gitignore.",
  "input_schema": {
    "type": "object",
    "required": ["pattern"],
    "properties": {
      "pattern": { "type": "string", "description": "Glob pattern (e.g. \"**/*.rs\", \"src/**/*.ts\")" },
      "path":    { "type": "string", "description": "Directory to search in (defaults to cwd)" }
    }
  }
}
```

#### Grep (read-only)

```json
{
  "name": "Grep",
  "description": "Search file contents using regex patterns. Supports output modes: 'content' (matching lines), 'files_with_matches' (file paths only), 'count' (match counts). Defaults to 'files_with_matches'.",
  "input_schema": {
    "type": "object",
    "required": ["pattern"],
    "properties": {
      "pattern":     { "type": "string",  "description": "Regex pattern to search for" },
      "path":        { "type": "string",  "description": "File or directory to search in (defaults to cwd)" },
      "output_mode": { "type": "string",  "enum": ["content", "files_with_matches", "count"], "description": "Output mode (default: files_with_matches)" },
      "glob":        { "type": "string",  "description": "Glob pattern to filter files (e.g. \"*.rs\")" },
      "head_limit":  { "type": "integer", "description": "Limit output to first N results" },
      "context":     { "type": "integer", "description": "Lines of context around matches (for content mode)" },
      "-i":          { "type": "boolean", "description": "Case-insensitive search" }
    }
  }
}
```

### Prompt Caching

Always enabled. Ephemeral `cache_control` markers are placed on:

1. The **system prompt** content block
2. The **last tool definition** in the tools array

```json
{ "cache_control": { "type": "ephemeral" } }
```

This allows the API to cache the system prompt and tool definitions across requests in the same session, reducing input token costs.

### Extended Thinking

Activated via `--thinking-budget <tokens>` CLI flag. When enabled:

- `temperature` is set to `1.0` (required by the API)
- `thinking` config is added to the request:

```json
{
  "thinking": {
    "type": "enabled",
    "budget_tokens": 5000
  },
  "temperature": 1.0
}
```

## Response (SSE Stream)

The API returns Server-Sent Events. Each event has an `event` type and JSON `data` payload.

### Event Sequence

A typical response follows this pattern:

```
message_start → [content_block_start → content_block_delta* → content_block_stop]+ → message_delta → message_stop
```

Ping events can appear at any point.

### Event Types

#### `message_start`

First event. Contains the message metadata and initial usage.

```json
{
  "type": "message_start",
  "message": {
    "id": "msg_abc123",
    "type": "message",
    "role": "assistant",
    "content": [],
    "model": "claude-sonnet-4-5-20250929",
    "stop_reason": null,
    "usage": {
      "input_tokens": 150,
      "output_tokens": 1,
      "cache_creation_input_tokens": 2400,
      "cache_read_input_tokens": 0
    }
  }
}
```

#### `content_block_start`

Signals the start of a new content block (text, tool_use, or thinking).

```json
{ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }
{ "type": "content_block_start", "index": 1, "content_block": { "type": "tool_use", "id": "toolu_abc", "name": "Read", "input": {} } }
{ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "" } }
```

#### `content_block_delta`

Incremental content within a block. Delta types:

| Delta type | Field | Description |
|------------|-------|-------------|
| `text_delta` | `text` | Streamed text output |
| `input_json_delta` | `partial_json` | Streamed tool input JSON |
| `thinking_delta` | `thinking` | Streamed thinking text |
| `signature_delta` | `signature` | Thinking block signature |

```json
{ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Hello" } }
{ "type": "content_block_delta", "index": 1, "delta": { "type": "input_json_delta", "partial_json": "{\"file_path\":" } }
```

#### `content_block_stop`

Signals the end of a content block.

```json
{ "type": "content_block_stop", "index": 0 }
```

#### `message_delta`

Final stop reason and output token usage.

```json
{
  "type": "message_delta",
  "delta": { "stop_reason": "end_turn" },
  "usage": { "output_tokens": 42 }
}
```

Stop reasons: `end_turn`, `tool_use`, `max_tokens`, `stop_sequence`

#### `message_stop`

Stream is complete.

```json
{ "type": "message_stop" }
```

#### `ping`

Keep-alive. No meaningful data.

#### `error`

Server-side error during streaming.

```json
{
  "type": "error",
  "error": { "type": "overloaded_error", "message": "Overloaded" }
}
```

## Agent Loop

Chet runs up to **50** consecutive API calls in a tool-use loop:

1. Send `CreateMessageRequest` with messages + tools
2. Stream the response, collecting content blocks
3. If the response contains `tool_use` blocks and `stop_reason` is `tool_use`:
   - Check permissions for each tool
   - Execute permitted tools
   - Append `tool_result` blocks as a `user` message
   - Go to step 1
4. If `stop_reason` is `end_turn` or no tool calls: done

## Error Handling

HTTP error responses are classified by status code:

| Status | Error type | Description |
|--------|-----------|-------------|
| 400 | `BadRequest` | Invalid request parameters |
| 401 | `Auth` | Invalid or missing API key |
| 429 | `RateLimited` | Too many requests |
| 529 | `Overloaded` | API is overloaded |
| 500-599 | `Server` | Server error |

Error response body format:

```json
{
  "error": {
    "type": "invalid_request_error",
    "message": "max_tokens: 16384 > 8192, which is the maximum..."
  }
}
```

## Defaults

| Setting | Default | Override |
|---------|---------|---------|
| Model | `claude-sonnet-4-5-20250929` | `--model`, `CHET_MODEL`, config |
| Max tokens | `16384` | `--max-tokens`, config |
| Base URL | `https://api.anthropic.com` | `ANTHROPIC_API_BASE_URL`, config |
| API version | `2023-06-01` | Not configurable |
| Stream | `true` | Not configurable |
| Temperature | `null` (API default) | `1.0` when thinking is enabled |
| Tool loop limit | 50 | Not configurable |
