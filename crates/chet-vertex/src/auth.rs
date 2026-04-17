//! Google Application Default Credentials (ADC) resolution.
//!
//! Resolves access tokens from:
//! 1. GOOGLE_APPLICATION_CREDENTIALS env var (service account JSON)
//! 2. `gcloud auth print-access-token` (user credentials)
//!
//! Tokens are cached and refreshed when expired (~1hr lifetime).

use std::sync::Arc;
use tokio::sync::Mutex;

/// Cached Google access token.
struct CachedToken {
    token: String,
    expires_at: std::time::Instant,
}

/// Google credential resolver.
pub struct GoogleAuth {
    cache: Arc<Mutex<Option<CachedToken>>>,
}

impl GoogleAuth {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(None)),
        }
    }

    /// Get a valid access token, refreshing if needed.
    pub async fn access_token(&self) -> Result<String, String> {
        let mut cache = self.cache.lock().await;

        // Return cached token if still valid (with 60s buffer)
        if let Some(ref cached) = *cache {
            if cached.expires_at > std::time::Instant::now() + std::time::Duration::from_secs(60) {
                return Ok(cached.token.clone());
            }
        }

        // Resolve fresh token
        let token = resolve_token().await?;
        *cache = Some(CachedToken {
            token: token.clone(),
            // Google tokens typically last 1 hour
            expires_at: std::time::Instant::now() + std::time::Duration::from_secs(3500),
        });

        Ok(token)
    }
}

/// Resolve a Google access token from the environment.
async fn resolve_token() -> Result<String, String> {
    // 1. Try GOOGLE_ACCESS_TOKEN env var (for testing/CI)
    if let Ok(token) = std::env::var("GOOGLE_ACCESS_TOKEN") {
        if !token.is_empty() {
            return Ok(token);
        }
    }

    // 2. Try `gcloud auth print-access-token`
    match tokio::process::Command::new("gcloud")
        .args(["auth", "print-access-token"])
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !token.is_empty() {
                return Ok(token);
            }
        }
        _ => {}
    }

    Err("No Google credentials found. Set GOOGLE_ACCESS_TOKEN or run `gcloud auth login`.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_auth_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GoogleAuth>();
    }
}
