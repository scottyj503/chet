//! Message types for the Anthropic Messages API.

use serde::{Deserialize, Serialize};

/// Role of a message participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// A single message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

/// A block of content within a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: Vec<ToolResultContent>,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    Image {
        source: ImageSource,
    },
}

/// Content within a tool result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolResultContent {
    Text { text: String },
    Image { source: ImageSource },
}

/// Source of an image in a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: ImageSourceType,
    pub media_type: String,
    pub data: String,
}

/// How an image is provided.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageSourceType {
    Base64,
    Url,
}

/// Reason the model stopped generating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
}

/// Cache control marker for prompt caching.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub cache_type: String,
}

impl CacheControl {
    /// Create an ephemeral cache control marker.
    pub fn ephemeral() -> Self {
        Self {
            cache_type: "ephemeral".to_string(),
        }
    }
}

/// A content block within the system prompt (supports cache_control).
#[derive(Debug, Clone, Serialize)]
pub struct SystemContent {
    #[serde(rename = "type")]
    pub content_type: &'static str,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

/// Effort level for extended thinking — syntactic sugar over `budget_tokens`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
}

impl Effort {
    /// Map effort level to a thinking token budget.
    pub fn budget_tokens(self) -> u32 {
        match self {
            Effort::Low => 1024,
            Effort::Medium => 8192,
            Effort::High => 32768,
        }
    }
}

impl std::fmt::Display for Effort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Effort::Low => write!(f, "low"),
            Effort::Medium => write!(f, "medium"),
            Effort::High => write!(f, "high"),
        }
    }
}

impl std::str::FromStr for Effort {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "low" => Ok(Effort::Low),
            "medium" | "med" => Ok(Effort::Medium),
            "high" => Ok(Effort::High),
            _ => Err(format!(
                "unknown effort level: {s} (use low, medium, or high)"
            )),
        }
    }
}

/// Configuration for extended thinking.
#[derive(Debug, Clone, Serialize)]
pub struct ThinkingConfig {
    #[serde(rename = "type")]
    pub thinking_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
}

/// Token usage information from an API response.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

impl Usage {
    /// Accumulate usage from another response.
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
    }
}

/// A request to the Anthropic Messages API.
#[derive(Debug, Clone, Serialize)]
pub struct CreateMessageRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<Vec<SystemContent>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    pub stream: bool,
}

/// A tool definition sent to the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

/// A response from the Anthropic Messages API.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateMessageResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub response_type: String,
    pub role: Role,
    pub content: Vec<ContentBlock>,
    pub model: String,
    pub stop_reason: Option<StopReason>,
    pub usage: Usage,
}

/// SSE stream events from the Messages API.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    MessageStart {
        message: CreateMessageResponse,
    },
    ContentBlockStart {
        index: usize,
        content_block: ContentBlock,
    },
    ContentBlockDelta {
        index: usize,
        delta: ContentDelta,
    },
    ContentBlockStop {
        index: usize,
    },
    MessageDelta {
        delta: MessageDelta,
        usage: Option<Usage>,
    },
    MessageStop,
    Ping,
    Error {
        error: ApiErrorResponse,
    },
}

/// A delta within a content block stream.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    ThinkingDelta { thinking: String },
    SignatureDelta { signature: String },
}

/// Delta for message-level changes.
#[derive(Debug, Clone, Deserialize)]
pub struct MessageDelta {
    pub stop_reason: Option<StopReason>,
}

/// Error response body from the API.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiErrorResponse {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_content_with_cache_control() {
        let system = vec![SystemContent {
            content_type: "text",
            text: "You are helpful.".to_string(),
            cache_control: Some(CacheControl::ephemeral()),
        }];
        let json = serde_json::to_value(&system).unwrap();
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[0]["text"], "You are helpful.");
        assert_eq!(arr[0]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn test_system_content_without_cache_control() {
        let system = vec![SystemContent {
            content_type: "text",
            text: "Hello".to_string(),
            cache_control: None,
        }];
        let json = serde_json::to_value(&system).unwrap();
        let arr = json.as_array().unwrap();
        assert!(arr[0].get("cache_control").is_none());
    }

    #[test]
    fn test_tool_definition_with_cache_control() {
        let tool = ToolDefinition {
            name: "Read".to_string(),
            description: "Read a file".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            cache_control: Some(CacheControl::ephemeral()),
        };
        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn test_tool_definition_without_cache_control() {
        let tool = ToolDefinition {
            name: "Read".to_string(),
            description: "Read a file".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            cache_control: None,
        };
        let json = serde_json::to_value(&tool).unwrap();
        assert!(json.get("cache_control").is_none());
    }

    #[test]
    fn test_thinking_config_serialization() {
        let config = ThinkingConfig {
            thinking_type: "enabled".to_string(),
            budget_tokens: Some(10000),
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["type"], "enabled");
        assert_eq!(json["budget_tokens"], 10000);
    }

    #[test]
    fn test_request_with_thinking_and_system() {
        let req = CreateMessageRequest {
            model: "claude-sonnet-4-5-20250929".to_string(),
            max_tokens: 4096,
            messages: vec![],
            system: Some(vec![SystemContent {
                content_type: "text",
                text: "system".to_string(),
                cache_control: Some(CacheControl::ephemeral()),
            }]),
            tools: None,
            stop_sequences: None,
            temperature: Some(1.0),
            thinking: Some(ThinkingConfig {
                thinking_type: "enabled".to_string(),
                budget_tokens: Some(5000),
            }),
            stream: true,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(json["system"].is_array());
        assert_eq!(json["thinking"]["type"], "enabled");
        assert_eq!(json["thinking"]["budget_tokens"], 5000);
        assert_eq!(json["temperature"], 1.0);
    }

    #[test]
    fn test_effort_from_str_all_variants() {
        assert_eq!("low".parse::<Effort>().unwrap(), Effort::Low);
        assert_eq!("medium".parse::<Effort>().unwrap(), Effort::Medium);
        assert_eq!("med".parse::<Effort>().unwrap(), Effort::Medium);
        assert_eq!("high".parse::<Effort>().unwrap(), Effort::High);
        assert_eq!("HIGH".parse::<Effort>().unwrap(), Effort::High);
        assert!("invalid".parse::<Effort>().is_err());
    }

    #[test]
    fn test_effort_budget_tokens() {
        assert_eq!(Effort::Low.budget_tokens(), 1024);
        assert_eq!(Effort::Medium.budget_tokens(), 8192);
        assert_eq!(Effort::High.budget_tokens(), 32768);
    }

    #[test]
    fn test_effort_display() {
        assert_eq!(Effort::Low.to_string(), "low");
        assert_eq!(Effort::Medium.to_string(), "medium");
        assert_eq!(Effort::High.to_string(), "high");
    }

    #[test]
    fn test_effort_serde_roundtrip() {
        let json = serde_json::to_string(&Effort::High).unwrap();
        assert_eq!(json, "\"high\"");
        let parsed: Effort = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Effort::High);
    }

    #[test]
    fn test_request_without_thinking() {
        let req = CreateMessageRequest {
            model: "test".to_string(),
            max_tokens: 4096,
            messages: vec![],
            system: None,
            tools: None,
            stop_sequences: None,
            temperature: None,
            thinking: None,
            stream: true,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("thinking").is_none());
        assert!(json.get("system").is_none());
        assert!(json.get("temperature").is_none());
    }
}
