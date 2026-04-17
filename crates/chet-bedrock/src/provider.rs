//! BedrockProvider — implements the Provider trait for AWS Bedrock.

use crate::eventstream::EventStreamParser;
use aws_credential_types::provider::ProvideCredentials;
use chet_types::sse::parse_stream_event;
use chet_types::{
    ApiError, CreateMessageRequest, StreamEvent,
    provider::{EventStream, Provider},
};
use futures_util::StreamExt;
use std::future::Future;
use std::pin::Pin;

/// AWS Bedrock provider for Claude models.
pub struct BedrockProvider {
    region: String,
    client: reqwest::Client,
    credentials_cache: std::sync::Arc<tokio::sync::Mutex<Option<CachedCredentials>>>,
}

struct CachedCredentials {
    access_key: String,
    secret_key: String,
    session_token: Option<String>,
}

impl BedrockProvider {
    /// Create a new BedrockProvider. Credentials are resolved lazily from the
    /// standard AWS credential chain (env vars, profile, IMDS, etc.).
    pub fn new(region: &str) -> Self {
        Self {
            region: region.to_string(),
            client: reqwest::Client::new(),
            credentials_cache: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Build the Bedrock invoke URL for a model.
    fn invoke_url(&self, model_id: &str) -> String {
        format!(
            "https://bedrock-runtime.{}.amazonaws.com/model/{}/invoke-with-response-stream",
            self.region, model_id
        )
    }

    /// Resolve AWS credentials from the environment.
    async fn resolve_credentials(&self) -> Result<CachedCredentials, ApiError> {
        // Check env vars first (fastest path)
        let access_key = std::env::var("AWS_ACCESS_KEY_ID").ok();
        let secret_key = std::env::var("AWS_SECRET_ACCESS_KEY").ok();
        let session_token = std::env::var("AWS_SESSION_TOKEN").ok();

        if let (Some(ak), Some(sk)) = (access_key, secret_key) {
            return Ok(CachedCredentials {
                access_key: ak,
                secret_key: sk,
                session_token,
            });
        }

        // Fall back to aws-config SDK for profile, IMDS, ECS, SSO, STS
        let sdk_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let creds = sdk_config
            .credentials_provider()
            .ok_or(ApiError::Auth {
                message: "No AWS credentials provider found".to_string(),
            })?
            .provide_credentials()
            .await
            .map_err(|e| ApiError::Auth {
                message: format!("Failed to resolve AWS credentials: {e}"),
            })?;

        Ok(CachedCredentials {
            access_key: creds.access_key_id().to_string(),
            secret_key: creds.secret_access_key().to_string(),
            session_token: creds.session_token().map(|s| s.to_string()),
        })
    }
}

impl Provider for BedrockProvider {
    fn name(&self) -> &str {
        "bedrock"
    }

    fn create_message_stream<'a>(
        &'a self,
        request: &'a CreateMessageRequest,
    ) -> Pin<Box<dyn Future<Output = Result<EventStream, ApiError>> + Send + 'a>> {
        Box::pin(async move {
            let url = self.invoke_url(&request.model);

            // Build the request body with Bedrock-specific anthropic_version
            let mut body_value =
                serde_json::to_value(request).map_err(|e| ApiError::BadRequest {
                    message: format!("Failed to serialize request: {e}"),
                })?;
            body_value["anthropic_version"] = serde_json::json!("bedrock-2023-05-31");
            let body_bytes = serde_json::to_vec(&body_value).map_err(|e| ApiError::BadRequest {
                message: format!("Failed to serialize request: {e}"),
            })?;

            // Resolve credentials
            let creds = self.resolve_credentials().await?;

            // Sign with SigV4
            let datetime = chrono::Utc::now();
            let date_str = datetime.format("%Y%m%dT%H%M%SZ").to_string();
            let date_short = datetime.format("%Y%m%d").to_string();

            let host = format!("bedrock-runtime.{}.amazonaws.com", self.region);
            let content_hash = sha256_hex(&body_bytes);

            let signed_headers = "content-type;host;x-amz-content-sha256;x-amz-date";
            let canonical_headers = format!(
                "content-type:application/json\nhost:{host}\nx-amz-content-sha256:{content_hash}\nx-amz-date:{date_str}\n"
            );

            let canonical_request = format!(
                "POST\n/model/{}/invoke-with-response-stream\n\n{canonical_headers}\n{signed_headers}\n{content_hash}",
                request.model
            );

            let credential_scope = format!("{date_short}/{}/bedrock/aws4_request", self.region);
            let string_to_sign = format!(
                "AWS4-HMAC-SHA256\n{date_str}\n{credential_scope}\n{}",
                sha256_hex(canonical_request.as_bytes())
            );

            let signing_key =
                derive_signing_key(&creds.secret_key, &date_short, &self.region, "bedrock");
            let signature = hmac_sha256_hex(&signing_key, string_to_sign.as_bytes());

            let authorization = format!(
                "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}",
                creds.access_key
            );

            let mut req = self
                .client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("x-amz-date", &date_str)
                .header("x-amz-content-sha256", &content_hash)
                .header("Authorization", &authorization)
                .body(body_bytes);

            if let Some(ref token) = creds.session_token {
                req = req.header("x-amz-security-token", token);
            }

            let response = req.send().await.map_err(|e| {
                if e.is_timeout() {
                    ApiError::Timeout
                } else {
                    ApiError::Network(e.to_string())
                }
            })?;

            let status = response.status().as_u16();
            if status != 200 {
                let body = response.text().await.unwrap_or_default();
                return Err(match status {
                    401 | 403 => ApiError::Auth {
                        message: format!("Bedrock auth error ({status}): {body}"),
                    },
                    429 => ApiError::RateLimited {
                        retry_after_ms: None,
                    },
                    529 => ApiError::Overloaded,
                    s if s >= 500 => ApiError::Server {
                        status: s,
                        message: body,
                    },
                    _ => ApiError::BadRequest { message: body },
                });
            }

            // Convert EventStream binary frames to StreamEvents
            let byte_stream = response.bytes_stream();
            let event_stream = byte_stream
                .scan(EventStreamParser::new(), |parser, chunk| {
                    let events: Vec<Result<StreamEvent, ApiError>> = match chunk {
                        Ok(bytes) => parser
                            .feed(&bytes)
                            .into_iter()
                            .filter_map(|msg| {
                                let payload = msg.payload_str()?.to_string();
                                // Bedrock wraps events in {"bytes":"<base64>"} format
                                let inner_json =
                                    extract_bedrock_payload(&payload).unwrap_or(payload);
                                // Extract the event type from the JSON "type" field
                                let event_type: Option<String> =
                                    serde_json::from_str::<serde_json::Value>(&inner_json)
                                        .ok()
                                        .and_then(|v| {
                                            v.get("type").and_then(|t| t.as_str()).map(String::from)
                                        });
                                match parse_stream_event(&event_type, &inner_json) {
                                    Ok(Some(event)) => Some(Ok(event)),
                                    Ok(None) => None,
                                    Err(e) => Some(Err(e)),
                                }
                            })
                            .collect(),
                        Err(e) => vec![Err(ApiError::Network(e.to_string()))],
                    };
                    std::future::ready(Some(futures_util::stream::iter(events)))
                })
                .flatten();

            Ok(Box::pin(event_stream) as EventStream)
        })
    }
}

/// Extract the inner JSON from Bedrock's `{"bytes":"<base64>"}` wrapper.
fn extract_bedrock_payload(payload: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let b64 = value.get("bytes")?.as_str()?;
    base64_decode_to_string(b64).ok()
}

/// Simple base64 decode to String.
fn base64_decode_to_string(input: &str) -> Result<String, String> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for &byte in input.as_bytes() {
        if byte == b'=' || byte == b'\n' || byte == b'\r' || byte == b' ' {
            continue;
        }
        let val = TABLE
            .iter()
            .position(|&b| b == byte)
            .ok_or("Invalid base64")? as u32;
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    String::from_utf8(output).map_err(|e| e.to_string())
}

/// SHA-256 hash as hex string.
fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(data);
    hex_encode(&hash)
}

/// HMAC-SHA256 as hex string.
fn hmac_sha256_hex(key: &[u8], data: &[u8]) -> String {
    hex_encode(&hmac_sha256(key, data))
}

/// HMAC-SHA256 as raw bytes.
fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Derive the SigV4 signing key.
fn derive_signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invoke_url_format() {
        let provider = BedrockProvider::new("us-east-1");
        let url = provider.invoke_url("anthropic.claude-sonnet-4-5-20250929-v1:0");
        assert!(url.contains("us-east-1"));
        assert!(url.contains("invoke-with-response-stream"));
        assert!(url.contains("anthropic.claude-sonnet"));
    }

    #[test]
    fn base64_decode_works() {
        let decoded = base64_decode_to_string("SGVsbG8gV29ybGQ=").unwrap();
        assert_eq!(decoded, "Hello World");
    }

    #[test]
    fn sha256_hex_works() {
        let hash = sha256_hex(b"hello");
        assert_eq!(hash.len(), 64); // 256 bits = 32 bytes = 64 hex chars
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn signing_key_derivation() {
        let key = derive_signing_key(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20150830",
            "us-east-1",
            "iam",
        );
        assert_eq!(key.len(), 32);
    }

    #[test]
    fn provider_name() {
        let provider = BedrockProvider::new("us-east-1");
        assert_eq!(provider.name(), "bedrock");
    }

    #[test]
    fn extract_bedrock_payload_works() {
        // Base64 of {"type":"ping"}
        let inner = r#"{"type":"ping"}"#;
        let b64 = base64_encode(inner.as_bytes());
        let wrapper = format!(r#"{{"bytes":"{b64}"}}"#);
        let result = extract_bedrock_payload(&wrapper).unwrap();
        assert_eq!(result, inner);
    }

    #[test]
    fn extract_bedrock_payload_not_wrapped() {
        let result = extract_bedrock_payload(r#"{"type":"ping"}"#);
        assert!(result.is_none()); // no "bytes" field
    }

    fn base64_encode(data: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut result = String::new();
        for chunk in data.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
            let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
            let triple = (b0 << 16) | (b1 << 8) | b2;
            result.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
            result.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
            if chunk.len() > 1 {
                result.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
            } else {
                result.push('=');
            }
            if chunk.len() > 2 {
                result.push(TABLE[(triple & 0x3F) as usize] as char);
            } else {
                result.push('=');
            }
        }
        result
    }
}
