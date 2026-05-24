#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
//! # `pkdealer_agent_ollama` (library)
//!
//! Provides [`OllamaBackend`], an implementation of
//! [`pkdealer_agent_llm::LlmBackend`] that targets a locally-running Ollama
//! server (`/api/chat` endpoint). The binary in `main.rs` wires this backend
//! into a [`pkdealer_agent_llm::LlmPokerAgent`] and hands the result to
//! `pkdealer_agent_core::run_agent`.
//!
//! Ollama is unauthenticated by default and listens on
//! `http://localhost:11434`, so this backend skips the API-key handling that
//! the Anthropic backend needs. Token counts are read from the Ollama
//! response's `prompt_eval_count` / `eval_count` fields and mapped to the
//! generic [`pkdealer_agent_llm::LlmResponse::input_tokens`] /
//! [`pkdealer_agent_llm::LlmResponse::output_tokens`] fields.

use async_trait::async_trait;
use pkdealer_agent_llm::{LlmBackend, LlmError, LlmResponse};

/// `LlmBackend` implementation that talks to an Ollama server.
///
/// Posts a single-turn chat completion to `{host}/api/chat` with
/// `stream: false`. No authentication header is sent — Ollama is expected
/// to be reachable directly.
///
/// # Examples
///
/// ```rust
/// use pkdealer_agent_ollama::OllamaBackend;
///
/// let backend = OllamaBackend::new(
///     "http://localhost:11434".to_string(),
///     "llama3.1".to_string(),
/// );
/// assert_eq!(backend.model(), "llama3.1");
/// assert_eq!(backend.host(), "http://localhost:11434");
/// ```
pub struct OllamaBackend {
    client: reqwest::Client,
    host: String,
    model: String,
}

impl OllamaBackend {
    /// Construct a backend pointed at an Ollama HTTP host.
    ///
    /// The host should include scheme and port (e.g. `http://localhost:11434`).
    #[must_use]
    pub fn new(host: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            host,
            model,
        }
    }

    /// The Ollama model identifier this backend will send on every request.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// The Ollama HTTP host this backend posts to.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }
}

#[derive(serde::Serialize)]
struct ApiRequest<'a> {
    model: &'a str,
    messages: Vec<ApiMessage<'a>>,
    stream: bool,
}

#[derive(serde::Serialize)]
struct ApiMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(serde::Deserialize)]
struct ApiResponse {
    message: ApiResponseMessage,
    #[serde(default)]
    prompt_eval_count: u32,
    #[serde(default)]
    eval_count: u32,
}

#[derive(serde::Deserialize)]
struct ApiResponseMessage {
    #[serde(default)]
    content: String,
}

#[async_trait]
impl LlmBackend for OllamaBackend {
    async fn complete(&self, prompt: &str) -> Result<LlmResponse, LlmError> {
        let body = ApiRequest {
            model: &self.model,
            messages: vec![ApiMessage {
                role: "user",
                content: prompt,
            }],
            stream: false,
        };

        let url = format!("{}/api/chat", self.host);
        let resp = self
            .client
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::new(format!("request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(LlmError::new(format!("Ollama API {status}: {text}")));
        }

        let parsed: ApiResponse = resp
            .json()
            .await
            .map_err(|e| LlmError::new(format!("response parse failed: {e}")))?;

        Ok(LlmResponse {
            text: parsed.message.content,
            input_tokens: parsed.prompt_eval_count,
            output_tokens: parsed.eval_count,
        })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn new_sets_fields() {
        let backend =
            OllamaBackend::new("http://localhost:11434".to_string(), "llama3.1".to_string());
        assert_eq!(backend.host(), "http://localhost:11434");
        assert_eq!(backend.model(), "llama3.1");
    }

    #[tokio::test]
    async fn complete_returns_error_when_server_unreachable() {
        let backend = OllamaBackend::new("http://127.0.0.1:1".to_string(), "llama3.1".to_string());
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

    #[tokio::test]
    async fn complete_maps_response_fields() {
        // Spin up a tiny tokio TCP listener that returns one canned HTTP
        // response, then point the backend at it. Verifies that
        // prompt_eval_count → input_tokens and eval_count → output_tokens.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("accept");
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let body = r#"{"message":{"role":"assistant","content":"raise 200"},"prompt_eval_count":120,"eval_count":7}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(response.as_bytes()).await;
            let _ = sock.shutdown().await;
        });

        let backend = OllamaBackend::new(format!("http://{addr}"), "llama3.1".to_string());
        let response = backend.complete("hi").await.expect("mock server succeeds");
        server.await.expect("server task");

        assert_eq!(response.text, "raise 200");
        assert_eq!(response.input_tokens, 120);
        assert_eq!(response.output_tokens, 7);
    }
}
