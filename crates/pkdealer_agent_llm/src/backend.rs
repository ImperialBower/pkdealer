//! The [`LlmBackend`] trait and its supporting types.
//!
//! A backend is responsible for the model-specific concerns: HTTP transport,
//! authentication, request and response serialization. All poker-side logic
//! (prompt construction, response parsing, fallbacks) is provided by the
//! generic [`crate::LlmPokerAgent`] wrapper, so a backend only needs to map
//! a single prompt string to a single text completion plus token counts.

use async_trait::async_trait;

/// A model-specific text-completion endpoint.
///
/// Implementations encapsulate everything a single LLM provider needs:
/// HTTP client, base URL, authentication, request body shape, response
/// parsing. The poker agent layer above calls [`complete`](LlmBackend::complete)
/// once per decision and converts the returned text into a
/// [`pkdealer_agent_core::Decision`].
///
/// # Examples
///
/// ```rust
/// use async_trait::async_trait;
/// use pkdealer_agent_llm::{LlmBackend, LlmError, LlmResponse};
///
/// struct ConstantBackend(&'static str);
///
/// #[async_trait]
/// impl LlmBackend for ConstantBackend {
///     async fn complete(&self, _prompt: &str) -> Result<LlmResponse, LlmError> {
///         Ok(LlmResponse {
///             text: self.0.to_string(),
///             input_tokens: 0,
///             output_tokens: 0,
///         })
///     }
/// }
///
/// # #[tokio::main]
/// # async fn main() {
/// let backend = ConstantBackend("fold");
/// let response = backend.complete("prompt").await.expect("constant backend never errors");
/// assert_eq!(response.text, "fold");
/// # }
/// ```
#[async_trait]
pub trait LlmBackend: Send + Sync {
    /// Send `prompt` to the underlying model and return the text response
    /// together with input/output token counts.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError`] when the HTTP request fails, the server returns
    /// a non-success status, or the response body cannot be parsed.
    async fn complete(&self, prompt: &str) -> Result<LlmResponse, LlmError>;
}

/// A successful completion from an [`LlmBackend`].
///
/// `input_tokens` and `output_tokens` are recorded on the per-decision span
/// using `gen_ai.usage.*` OpenTelemetry semantic-convention attributes.
///
/// # Examples
///
/// ```rust
/// use pkdealer_agent_llm::LlmResponse;
///
/// let response = LlmResponse {
///     text: "raise 200".to_string(),
///     input_tokens: 120,
///     output_tokens: 3,
/// };
/// assert_eq!(response.text, "raise 200");
/// assert_eq!(response.input_tokens, 120);
/// ```
#[derive(Debug, Clone)]
pub struct LlmResponse {
    /// The raw text the model produced.
    pub text: String,
    /// Tokens consumed by the prompt.
    pub input_tokens: u32,
    /// Tokens produced in the completion.
    pub output_tokens: u32,
}

/// Error returned by an [`LlmBackend::complete`] call.
///
/// Backends produce free-form error messages — the wrapper agent only cares
/// that a request failed, not why, because the fallback is the same in
/// every case.
///
/// # Examples
///
/// ```rust
/// use pkdealer_agent_llm::LlmError;
///
/// let err = LlmError::new("connection refused");
/// assert!(err.to_string().contains("connection refused"));
/// ```
#[derive(Debug, Clone)]
pub struct LlmError {
    message: String,
}

impl LlmError {
    /// Construct a new error with the given message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LlmError {}

impl From<String> for LlmError {
    fn from(message: String) -> Self {
        Self { message }
    }
}

impl From<&str> for LlmError {
    fn from(message: &str) -> Self {
        Self {
            message: message.to_string(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn llm_response_fields_round_trip() {
        let response = LlmResponse {
            text: "call".to_string(),
            input_tokens: 42,
            output_tokens: 7,
        };
        let cloned = response.clone();
        assert_eq!(cloned.text, "call");
        assert_eq!(cloned.input_tokens, 42);
        assert_eq!(cloned.output_tokens, 7);
    }

    #[test]
    fn llm_error_display_matches_message() {
        let err = LlmError::new("boom");
        assert_eq!(err.to_string(), "boom");
    }

    #[test]
    fn llm_error_from_string() {
        let err: LlmError = "owned".to_string().into();
        assert_eq!(err.to_string(), "owned");
    }

    #[test]
    fn llm_error_from_str() {
        let err: LlmError = "borrowed".into();
        assert_eq!(err.to_string(), "borrowed");
    }

    #[tokio::test]
    async fn trait_object_invocation() {
        struct Stub;

        #[async_trait]
        impl LlmBackend for Stub {
            async fn complete(&self, prompt: &str) -> Result<LlmResponse, LlmError> {
                Ok(LlmResponse {
                    text: prompt.to_string(),
                    input_tokens: 1,
                    output_tokens: 1,
                })
            }
        }

        let backend: &dyn LlmBackend = &Stub;
        let response = backend.complete("ping").await.expect("stub succeeds");
        assert_eq!(response.text, "ping");
    }
}
