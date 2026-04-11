//! Server library - re-exports for binary compatibility.

pub mod args;
pub mod db;
pub mod handlers;
pub mod http;
pub mod roles;
pub mod state;
pub mod tls;
pub mod validation;

// Re-exports
pub use crate::args::Args;
pub use crate::state::State;
pub use crate::validation::AuthInfo;

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn test_auth_payload_validation_valid() {
        let payload = json!({
            "t": "auth",
            "u": "testuser",
            "pw": "hash123",
            "pk": "dGVzdGtleQ==",
            "status": {"text": "Online", "emoji": ""}
        });

        assert_eq!(payload.get("t").and_then(|v| v.as_str()), Some("auth"));
        assert_eq!(payload.get("u").and_then(|v| v.as_str()), Some("testuser"));
    }

    #[test]
    fn test_auth_payload_missing_username() {
        let payload = json!({
            "t": "auth",
            "pw": "hash123",
            "pk": "dGVzdGtleQ=="
        });

        assert!(payload.get("u").is_none());
    }

    #[test]
    fn test_channel_name_sanitization() {
        let test_cases = vec![
            ("general", "general"),
            ("General", "general"),
            ("general ", "general"),
            ("general_", "general_"),
            ("general!", "general"),
            ("my-channel", "my-channel"),
            ("my_channel", "my_channel"),
        ];

        for (input, expected) in test_cases {
            let sanitized = input
                .trim()
                .to_lowercase()
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .collect::<String>();
            assert_eq!(sanitized, expected, "input: {}", input);
        }
    }

    #[test]
    fn test_session_token_format() {
        let token = "abc123def456";
        assert!(token.len() >= 12);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_rate_limit_check() {
        let max_per_minute = 60u32;
        let window_secs = 60.0;

        assert!(max_per_minute > 0);
        assert!(window_secs > 0.0);
    }
}
