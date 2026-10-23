//! Client for OpenAI-style `chat/completions` APIs with SSE streaming: OpenRouter, and
//! any compatible endpoint (OpenAI, Ollama, LM Studio, ...). Also home to the error and
//! HTTP helpers shared with the other providers.
use std::fmt;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::sse::{LineBuffer, SseLine, parse_line};

pub const BASE_URL: &str = "https://openrouter.ai/api/v1";
const APP_TITLE: &str = "rsnotebook";
const APP_REFERER: &str = "https://github.com/henriqk0/local-notebook";
const MAX_ERROR_BYTES: usize = 8192;
const MAX_MODELS_BYTES: usize = 16 * 1024 * 1024;
const MAX_SSE_LINE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub message: String,
    pub status: Option<u16>,
}

impl ApiError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status: None,
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

#[derive(Debug)]
pub enum StreamError {
    Cancelled,
    Api(ApiError),
}

impl From<ApiError> for StreamError {
    fn from(e: ApiError) -> Self {
        Self::Api(e)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }
}

/// The HTTP client every provider uses: no redirects (they could carry credentials
/// elsewhere), a short connect timeout and a per-read timeout.
pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(60))
        .build()
        .expect("static client configuration is valid")
}

/// A friendly message for an HTTP error status from `label` (the provider's name).
pub(crate) fn error_message(label: &str, status: u16, body: &str) -> String {
    match status {
        401 | 403 => format!(
            "Invalid API key or no access ({status}) for {label}. Press Ctrl+K to check the key."
        ),
        402 => format!("Insufficient credits (402). Add credits to your {label} account."),
        429 => "Rate limited (429). Please wait and try again.".into(),
        _ => format!(
            "{label} error {status}: {}",
            body.chars().take(300).collect::<String>()
        ),
    }
}

pub(crate) fn network(e: reqwest::Error) -> ApiError {
    // reqwest errors can embed the request URL but never the headers, so the key stays out.
    ApiError::new(format!("Network error: {e}"))
}

/// Read at most `limit` bytes so a hostile or broken server can't exhaust memory.
pub(crate) async fn read_bounded(
    resp: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, ApiError> {
    let mut data = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(network)?;
        let room = limit.saturating_sub(data.len());
        data.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if data.len() >= limit {
            break;
        }
    }
    Ok(data)
}

#[derive(Default)]
pub struct StreamState {
    finished: bool,
}

/// Handle one SSE `data:` payload. Returns the content delta, if any.
pub fn handle_data(
    label: &str,
    payload: &str,
    state: &mut StreamState,
) -> Result<Option<String>, ApiError> {
    let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(payload) else {
        return Ok(None);
    };
    if let Some(err) = obj
        .get("error")
        .filter(|e| !e.is_null() && *e != &json!(false))
    {
        let message = err
            .get("message")
            .and_then(Value::as_str)
            .map(String::from)
            .unwrap_or_else(|| err.to_string());
        return Err(ApiError::new(format!("{label} stream error: {message}")));
    }
    let Some(first) = obj
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first())
    else {
        return Ok(None);
    };
    if first
        .get("finish_reason")
        .is_some_and(|f| f.as_str().is_some_and(|s| !s.is_empty()))
    {
        state.finished = true;
    }
    Ok(first
        .get("delta")
        .and_then(|d| d.get("content"))
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
        .map(String::from))
}

pub struct Client {
    /// `None` for local endpoints that need no key.
    api_key: Option<String>,
    base_url: String,
    /// Provider name used in error messages.
    label: String,
    /// OpenRouter asks apps to identify themselves; other endpoints don't need it.
    app_headers: bool,
    http: reqwest::Client,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl Client {
    pub fn new(api_key: &str) -> Self {
        Self::with_base_url(api_key, BASE_URL)
    }

    /// OpenRouter (or a stand-in server) at `base_url`.
    pub fn with_base_url(api_key: &str, base_url: &str) -> Self {
        Self::build("OpenRouter", base_url, Some(api_key), true)
    }

    /// Any OpenAI-compatible endpoint; the key is optional (local servers).
    pub fn compatible(label: &str, base_url: &str, api_key: Option<&str>) -> Self {
        Self::build(label, base_url, api_key, false)
    }

    fn build(label: &str, base_url: &str, api_key: Option<&str>, app_headers: bool) -> Self {
        Self {
            api_key: api_key.filter(|k| !k.is_empty()).map(String::from),
            base_url: base_url.trim_end_matches('/').to_string(),
            label: label.to_string(),
            app_headers,
            http: http_client(),
        }
    }

    fn headers(&self) -> Result<HeaderMap, ApiError> {
        let mut h = HeaderMap::new();
        if let Some(key) = &self.api_key {
            let mut auth = HeaderValue::from_str(&format!("Bearer {key}"))
                .map_err(|_| ApiError::new("API key contains invalid characters."))?;
            auth.set_sensitive(true);
            h.insert(AUTHORIZATION, auth);
        }
        h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        h.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        if self.app_headers {
            h.insert("HTTP-Referer", HeaderValue::from_static(APP_REFERER));
            h.insert("X-Title", HeaderValue::from_static(APP_TITLE));
        }
        Ok(h)
    }

    pub async fn list_models(&self) -> Result<Vec<Value>, ApiError> {
        let resp = self
            .http
            .get(format!("{}/models", self.base_url))
            .headers(self.headers()?)
            .send()
            .await
            .map_err(network)?;
        let status = resp.status().as_u16();
        if status != 200 {
            let body = read_bounded(resp, MAX_ERROR_BYTES)
                .await
                .unwrap_or_default();
            return Err(ApiError {
                message: error_message(&self.label, status, &String::from_utf8_lossy(&body)),
                status: Some(status),
            });
        }
        let body = read_bounded(resp, MAX_MODELS_BYTES).await?;
        let malformed = || ApiError::new("Malformed model list response.");
        let data: Value = serde_json::from_slice(&body).map_err(|_| malformed())?;
        let list = data
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(malformed)?;
        Ok(list.iter().filter(|v| v.is_object()).cloned().collect())
    }

    /// Stream a chat completion, calling `on_token` for each content delta.
    ///
    /// Fails with `StreamError::Cancelled` as soon as `cancel` fires, and with an
    /// error if the stream ends without a `finish_reason` (a truncated answer).
    pub async fn stream_chat(
        &self,
        model: &str,
        messages: &[ChatMessage],
        cancel: &CancellationToken,
        mut on_token: impl FnMut(String),
    ) -> Result<(), StreamError> {
        let payload = json!({
            "model": model,
            "messages": messages.iter().map(|m| json!({"role": m.role, "content": m.content})).collect::<Vec<_>>(),
            "stream": true,
        });
        let request = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .headers(self.headers()?)
            .json(&payload)
            .send();
        let resp = tokio::select! {
            _ = cancel.cancelled() => return Err(StreamError::Cancelled),
            r = request => r.map_err(network)?,
        };
        let status = resp.status().as_u16();
        if status != 200 {
            let body = read_bounded(resp, MAX_ERROR_BYTES)
                .await
                .unwrap_or_default();
            return Err(ApiError {
                message: error_message(&self.label, status, &String::from_utf8_lossy(&body)),
                status: Some(status),
            }
            .into());
        }

        let mut stream = resp.bytes_stream();
        let mut lines = LineBuffer::new(MAX_SSE_LINE_BYTES);
        let mut state = StreamState::default();
        let too_long = || ApiError::new("Malformed stream: line too long.");

        'outer: loop {
            let next = tokio::select! {
                _ = cancel.cancelled() => return Err(StreamError::Cancelled),
                n = stream.next() => n,
            };
            let (batch, ended) = match next {
                None => (lines.finish().into_iter().collect::<Vec<_>>(), true),
                Some(Err(e)) => return Err(network(e).into()),
                Some(Ok(bytes)) => (lines.push(&bytes).map_err(|_| too_long())?, false),
            };
            for line in batch {
                match parse_line(&line) {
                    SseLine::Skip => {}
                    SseLine::Done => break 'outer,
                    SseLine::Data(p) => {
                        if let Some(tok) = handle_data(&self.label, &p, &mut state)? {
                            on_token(tok);
                        }
                    }
                }
            }
            if ended {
                break;
            }
        }
        if !state.finished {
            return Err(ApiError::new("The response stream ended unexpectedly.").into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_error_and_finish_handling() {
        let mut st = StreamState::default();
        assert_eq!(
            handle_data(
                "OpenRouter",
                r#"{"choices":[{"delta":{"content":"Hi"}}]}"#,
                &mut st
            )
            .unwrap()
            .as_deref(),
            Some("Hi")
        );
        assert!(!st.finished);
        assert_eq!(
            handle_data(
                "X",
                r#"{"choices":[{"delta":{"content":""},"finish_reason":"stop"}]}"#,
                &mut st
            )
            .unwrap(),
            None
        );
        assert!(st.finished);
        assert_eq!(handle_data("X", "not json", &mut st).unwrap(), None);
        assert_eq!(handle_data("X", "[1,2]", &mut st).unwrap(), None);
        assert_eq!(
            handle_data("X", r#"{"choices":[]}"#, &mut st).unwrap(),
            None
        );
        let e = handle_data("OpenRouter", r#"{"error":{"message":"boom"}}"#, &mut st).unwrap_err();
        assert_eq!(e.message, "OpenRouter stream error: boom");
    }

    #[test]
    fn error_messages() {
        assert!(error_message("OpenRouter", 401, "").contains("Invalid API key"));
        assert!(error_message("Ollama", 500, "x").starts_with("Ollama error 500"));
        assert!(error_message("OpenRouter", 402, "").contains("credits"));
        assert!(error_message("OpenRouter", 429, "").contains("Rate limited"));
        assert!(error_message("OpenRouter", 500, &"x".repeat(1000)).len() < 340);
    }

    #[test]
    fn debug_output_hides_the_key() {
        let c = Client::new("sk-super-secret");
        assert!(!format!("{c:?}").contains("sk-super-secret"));
    }
}
