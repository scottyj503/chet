use std::fmt;

/// How to authenticate with the Anthropic Messages API.
#[derive(Clone)]
pub enum AuthCredential {
    /// API key sent via `x-api-key` header.
    ApiKey(String),
    /// Bearer token sent via `Authorization: Bearer` header.
    AuthToken(String),
}

impl AuthCredential {
    pub fn method_label(&self) -> &'static str {
        match self {
            AuthCredential::ApiKey(_) => "api-key",
            AuthCredential::AuthToken(_) => "auth-token",
        }
    }
}

impl fmt::Debug for AuthCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthCredential::ApiKey(_) => f.debug_tuple("ApiKey").field(&"****").finish(),
            AuthCredential::AuthToken(_) => f.debug_tuple("AuthToken").field(&"****").finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_label() {
        assert_eq!(AuthCredential::ApiKey("k".into()).method_label(), "api-key");
        assert_eq!(
            AuthCredential::AuthToken("t".into()).method_label(),
            "auth-token"
        );
    }

    #[test]
    fn debug_masks_secrets() {
        let key = AuthCredential::ApiKey("sk-ant-secret".into());
        let debug = format!("{key:?}");
        assert!(!debug.contains("sk-ant-secret"));
        assert!(debug.contains("****"));
    }
}
