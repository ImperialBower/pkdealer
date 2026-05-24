#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
//! # `pkdealer_agent_claude` (library)
//!
//! Provides [`ClaudeBackend`], an implementation of
//! [`pkdealer_agent_llm::LlmBackend`] that targets the Anthropic Messages
//! API. The binary in `main.rs` wires this backend into a
//! [`pkdealer_agent_llm::LlmPokerAgent`] and hands the result to
//! `pkdealer_agent_core::run_agent`.
//!
//! Pulling the backend into a library makes it testable in isolation
//! (via mock-HTTP fixtures) and lets other tools embed Claude-driven
//! decision making without spawning the agent binary.

use async_trait::async_trait;
use pkdealer_agent_llm::{LlmBackend, LlmError, LlmResponse};

/// `LlmBackend` implementation that talks to Anthropic's Messages API.
///
/// Authentication uses the `x-api-key` header. The request is single-turn
/// (`messages: [{role: "user", content: <prompt>}]`) and non-streaming —
/// sufficient for short poker decisions and easy to mock in tests.
///
/// # Examples
///
/// ```rust
/// use pkdealer_agent_claude::ClaudeBackend;
///
/// let backend = ClaudeBackend::new(
///     "sk-test-key".to_string(),
///     "claude-sonnet-4-6".to_string(),
///     16,
/// );
/// assert_eq!(backend.model(), "claude-sonnet-4-6");
/// assert_eq!(backend.max_tokens(), 16);
/// ```
pub struct ClaudeBackend {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    max_tokens: u32,
}

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

impl ClaudeBackend {
    /// Construct a backend pointed at the public Anthropic API.
    #[must_use]
    pub fn new(api_key: String, model: String, max_tokens: u32) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
            api_key,
            model,
            max_tokens,
        }
    }

    /// Construct a backend pointed at a custom base URL.
    ///
    /// Used by integration tests to point at a local mock server.
    #[must_use]
    pub fn with_base_url(
        api_key: String,
        model: String,
        max_tokens: u32,
        base_url: String,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            api_key,
            model,
            max_tokens,
        }
    }

    /// The Claude model identifier this backend will send on every request.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// The maximum tokens this backend allows Claude to generate per call.
    #[must_use]
    pub fn max_tokens(&self) -> u32 {
        self.max_tokens
    }
}

#[derive(serde::Serialize)]
struct ApiRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: Vec<ApiMessage<'a>>,
}

#[derive(serde::Serialize)]
struct ApiMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(serde::Deserialize)]
struct ApiResponse {
    content: Vec<ContentBlock>,
    usage: ApiUsage,
}

#[derive(serde::Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

#[derive(serde::Deserialize)]
struct ApiUsage {
    input_tokens: u32,
    output_tokens: u32,
}

#[async_trait]
impl LlmBackend for ClaudeBackend {
    async fn complete(&self, prompt: &str) -> Result<LlmResponse, LlmError> {
        let body = ApiRequest {
            model: &self.model,
            max_tokens: self.max_tokens,
            messages: vec![ApiMessage {
                role: "user",
                content: prompt,
            }],
        };

        let url = format!("{}/v1/messages", self.base_url);
        let resp = self
            .client
            .post(url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::new(format!("request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(LlmError::new(format!("Anthropic API {status}: {text}")));
        }

        let parsed: ApiResponse = resp
            .json()
            .await
            .map_err(|e| LlmError::new(format!("response parse failed: {e}")))?;

        let text = parsed
            .content
            .into_iter()
            .find(|b| b.kind == "text")
            .and_then(|b| b.text)
            .unwrap_or_default();

        Ok(LlmResponse {
            text,
            input_tokens: parsed.usage.input_tokens,
            output_tokens: parsed.usage.output_tokens,
        })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn new_sets_fields() {
        let backend = ClaudeBackend::new("key".to_string(), "claude-sonnet-4-6".to_string(), 16);
        assert_eq!(backend.api_key, "key");
        assert_eq!(backend.model(), "claude-sonnet-4-6");
        assert_eq!(backend.max_tokens(), 16);
        assert_eq!(backend.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn with_base_url_overrides_default() {
        let backend = ClaudeBackend::with_base_url(
            "k".to_string(),
            "m".to_string(),
            8,
            "http://127.0.0.1:1234".to_string(),
        );
        assert_eq!(backend.base_url, "http://127.0.0.1:1234");
    }

    #[tokio::test]
    async fn complete_returns_error_when_server_unreachable() {
        // Port 1 is reserved and not bound; the connection refuses immediately.
        let backend = ClaudeBackend::with_base_url(
            "k".to_string(),
            "m".to_string(),
            8,
            "http://127.0.0.1:1".to_string(),
        );
        let err = backend
            .complete("hi")
            .await
            .expect_err("unreachable server must error");
        let msg = err.to_string();
        assert!(
            msg.contains("request failed"),
            "expected request failure message, got {msg}"
        );
    }
}
