//! Retry with exponential backoff for API requests.

use chet_types::ApiError;
use rand::Rng;

/// Configuration for retry behavior on transient API errors.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (0 = no retries).
    pub max_retries: u32,
    /// Initial delay in milliseconds before the first retry.
    pub initial_delay_ms: u64,
    /// Maximum delay in milliseconds between retries.
    pub max_delay_ms: u64,
    /// Multiplier applied to the delay after each attempt.
    pub backoff_factor: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 2,
            initial_delay_ms: 1000,
            max_delay_ms: 60_000,
            backoff_factor: 2.0,
        }
    }
}

/// Returns `true` if the error is transient and the request should be retried.
pub fn is_retryable(error: &ApiError) -> bool {
    matches!(
        error,
        ApiError::RateLimited { .. }
            | ApiError::Overloaded
            | ApiError::Server { .. }
            | ApiError::Network(_)
            | ApiError::Timeout
    )
}

/// Calculate the delay in milliseconds before the next retry attempt.
///
/// If `retry_after_ms` is provided (from the server's `Retry-After` header),
/// it is used directly (clamped to `max_delay_ms`). Otherwise, exponential
/// backoff is applied: `initial_delay_ms * backoff_factor^attempt` with
/// ±25% jitter, clamped to `max_delay_ms`.
pub fn calculate_delay(config: &RetryConfig, attempt: u32, retry_after_ms: Option<u64>) -> u64 {
    if let Some(server_delay) = retry_after_ms {
        return server_delay.min(config.max_delay_ms);
    }

    let base = config.initial_delay_ms as f64 * config.backoff_factor.powi(attempt as i32);
    let clamped = base.min(config.max_delay_ms as f64);

    // Apply ±25% jitter
    let jitter_factor = rand::rng().random_range(0.75..=1.25);
    let jittered = clamped * jitter_factor;

    (jittered as u64).min(config.max_delay_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 2);
        assert_eq!(config.initial_delay_ms, 1000);
        assert_eq!(config.max_delay_ms, 60_000);
        assert!((config.backoff_factor - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn is_retryable_rate_limited() {
        assert!(is_retryable(&ApiError::RateLimited {
            retry_after_ms: None,
        }));
    }

    #[test]
    fn is_retryable_overloaded() {
        assert!(is_retryable(&ApiError::Overloaded));
    }

    #[test]
    fn is_retryable_server_error() {
        assert!(is_retryable(&ApiError::Server {
            status: 500,
            message: "internal error".into(),
        }));
    }

    #[test]
    fn is_retryable_network_error() {
        assert!(is_retryable(&ApiError::Network(
            "connection refused".into()
        )));
    }

    #[test]
    fn is_retryable_timeout() {
        assert!(is_retryable(&ApiError::Timeout));
    }

    #[test]
    fn is_retryable_auth_error() {
        assert!(!is_retryable(&ApiError::Auth {
            message: "invalid key".into(),
        }));
    }

    #[test]
    fn is_retryable_bad_request() {
        assert!(!is_retryable(&ApiError::BadRequest {
            message: "bad input".into(),
        }));
    }

    #[test]
    fn is_retryable_stream_parse() {
        assert!(!is_retryable(&ApiError::StreamParse("bad json".into())));
    }

    #[test]
    fn calculate_delay_exponential() {
        let config = RetryConfig {
            max_retries: 5,
            initial_delay_ms: 1000,
            max_delay_ms: 60_000,
            backoff_factor: 2.0,
        };

        // Attempt 0: base = 1000 * 2^0 = 1000, with ±25% jitter → [750, 1250]
        let delay0 = calculate_delay(&config, 0, None);
        assert!((750..=1250).contains(&delay0), "delay0={delay0}");

        // Attempt 1: base = 1000 * 2^1 = 2000, with ±25% jitter → [1500, 2500]
        let delay1 = calculate_delay(&config, 1, None);
        assert!((1500..=2500).contains(&delay1), "delay1={delay1}");

        // Attempt 2: base = 1000 * 2^2 = 4000, with ±25% jitter → [3000, 5000]
        let delay2 = calculate_delay(&config, 2, None);
        assert!((3000..=5000).contains(&delay2), "delay2={delay2}");
    }

    #[test]
    fn calculate_delay_respects_retry_after() {
        let config = RetryConfig::default();

        // Server says wait 5 seconds
        let delay = calculate_delay(&config, 0, Some(5000));
        assert_eq!(delay, 5000);
    }

    #[test]
    fn calculate_delay_retry_after_capped() {
        let config = RetryConfig {
            max_delay_ms: 10_000,
            ..RetryConfig::default()
        };

        // Server says wait 30 seconds, but max is 10 seconds
        let delay = calculate_delay(&config, 0, Some(30_000));
        assert_eq!(delay, 10_000);
    }

    #[test]
    fn calculate_delay_capped_at_max() {
        let config = RetryConfig {
            max_retries: 10,
            initial_delay_ms: 1000,
            max_delay_ms: 5000,
            backoff_factor: 10.0,
        };

        // Attempt 5: base = 1000 * 10^5 = way over max
        let delay = calculate_delay(&config, 5, None);
        assert!(delay <= config.max_delay_ms, "delay={delay}");
    }
}
