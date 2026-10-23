//! Anthropic Messages API (API key) client with SSE streaming. `handle_event` is shared
//! with the `claude` CLI provider, which emits the same stream events.
use std::fmt;

use futures_util::StreamExt;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::openrouter::{
    ApiError, ChatMessage, StreamError, error_message, http_client, network, read_bounded,
};
use super::provider::Prompt;
use super::sse::{LineBuffer, SseLine, parse_line};

pub const BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";
/// The Messages API requires an output cap; this is within every current model's limit.
pub const MAX_TOKENS: u32 = 8192;
const LABEL: &str = "Anthropic";
const MAX_ERROR_BYTES: usize = 8192;
const MAX_MODELS_BYTES: usize = 8 * 1024 * 1024;
const MAX_SSE_LINE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
pub struct EventState {
    pub finished: bool,
}

/// Handle one stream event (`{"type": "content_block_delta", ...}`). Returns the text
/// delta, if any.
pub fn handle_event(
    label: &str,
    ev: &Value,
    state: &mut EventState,
) -> Result<Option<String>, ApiError> {
    match ev.get("type").and_then(Value::as_str) {
        Some("content_block_delta") => Ok(ev
            .get("delta")
            .filter(|d| d.get("type").and_then(Value::as_str) == Some("text_delta"))
            .and_then(|d| d.get("text"))
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(String::from)),
        Some("message_delta") => {
            if ev
                .pointer("/delta/stop_reason")
                .is_some_and(|r| r.as_str().is_some_and(|s| !s.is_empty()))
            {
                state.finished = true;
            }
            Ok(None)
        }
        Some("message_stop") => {
            state.finished = true;
            Ok(None)
        }
        Some("error") => {
            let message = ev
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            Err(ApiError {
                message: format!("{label} stream error: {message}"),
                status: None,
            })
        }
        _ => Ok(None),
    }
}

/// Make `history + question` valid for the Messages API: non-empty messages,
/// alternating roles, starting with a user message.
pub fn normalize_messages(history: &[ChatMessage], question: &str) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::new();
    let all = history
        .iter()
        .cloned()
        .chain(std::iter::once(ChatMessage::new("user", question)));
    for m in
        all.filter(|m| (m.role == "user" || m.role == "assistant") && !m.content.trim().is_empty())
    {
        match out.last_mut() {
            Some(last) if last.role == m.role => {
                last.content.push_str("\n\n");
                last.content.push_str(&m.content);
            }
            _ => out.push(m),
        }
    }
    while out.first().is_some_and(|m| m.role == "assistant") {
        out.remove(0);
    }
    out
}

/// The `error.message` of an Anthropic error body, or the raw text.
fn error_text(body: &[u8]) -> String {
    let raw = String::from_utf8_lossy(body);
    serde_json::from_str::<Value>(&raw)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(Value::as_str)
                .map(String::from)
        })
        .unwrap_or_else(|| raw.into_owned())
}

pub struct Client {
    api_key: String,
    base_url: String,
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

    pub fn with_base_url(api_key: &str, base_url: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            http: http_client(),
        }
    }

    fn headers(&self) -> Result<HeaderMap, ApiError> {
        let mut h = HeaderMap::new();
        let mut key = HeaderValue::from_str(&self.api_key).map_err(|_| ApiError {
            message: "API key contains invalid characters.".into(),
            status: None,
        })?;
        key.set_sensitive(true);
        h.insert("x-api-key", key);
        h.insert("anthropic-version", HeaderValue::from_static(API_VERSION));
        h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        h.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        Ok(h)
    }

    async fn http_error(resp: reqwest::Response) -> ApiError {
        let status = resp.status().as_u16();
        let body = read_bounded(resp, MAX_ERROR_BYTES)
            .await
            .unwrap_or_default();
        let detail = error_text(&body);
        let message = match status {
            401 | 403 | 402 | 429 => error_message(LABEL, status, &detail),
            _ => format!(
                "{LABEL} error {status}: {}",
                detail.chars().take(300).collect::<String>()
            ),
        };
        ApiError {
            message,
            status: Some(status),
        }
    }

    pub async fn list_models(&self) -> Result<Vec<Value>, ApiError> {
        let resp = self
            .http
            .get(format!("{}/v1/models?limit=1000", self.base_url))
            .headers(self.headers()?)
            .send()
            .await
            .map_err(network)?;
        if resp.status().as_u16() != 200 {
            return Err(Self::http_error(resp).await);
        }
        let body = read_bounded(resp, MAX_MODELS_BYTES).await?;
        let malformed = || ApiError {
            message: "Malformed model list response.".into(),
            status: None,
        };
        let data: Value = serde_json::from_slice(&body).map_err(|_| malformed())?;
        let list = data
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(malformed)?;
        Ok(list.iter().filter(|v| v.is_object()).cloned().collect())
    }

    pub async fn stream_chat(
        &self,
        model: &str,
        prompt: &Prompt,
        cancel: &CancellationToken,
        mut on_token: impl FnMut(String),
    ) -> Result<(), StreamError> {
        let payload = json!({
            "model": model,
            "max_tokens": MAX_TOKENS,
            "system": prompt.system_with_sources(),
            "messages": normalize_messages(&prompt.history, &prompt.question)
                .iter().map(|m| json!({"role": m.role, "content": m.content})).collect::<Vec<_>>(),
            "stream": true,
        });
        let request = self
            .http
            .post(format!("{}/v1/messages", self.base_url))
            .headers(self.headers()?)
            .json(&payload)
            .send();
        let resp = tokio::select! {
            _ = cancel.cancelled() => return Err(StreamError::Cancelled),
            r = request => r.map_err(network)?,
        };
        if resp.status().as_u16() != 200 {
            return Err(Self::http_error(resp).await.into());
        }

        let mut stream = resp.bytes_stream();
        let mut lines = LineBuffer::new(MAX_SSE_LINE_BYTES);
        let mut state = EventState::default();
        'outer: loop {
            let next = tokio::select! {
                _ = cancel.cancelled() => return Err(StreamError::Cancelled),
                n = stream.next() => n,
            };
            let (batch, ended) = match next {
                None => (lines.finish().into_iter().collect::<Vec<_>>(), true),
                Some(Err(e)) => return Err(network(e).into()),
                Some(Ok(bytes)) => (
                    lines.push(&bytes).map_err(|_| ApiError {
                        message: "Malformed stream: line too long.".into(),
                        status: None,
                    })?,
                    false,
                ),
            };
            for line in batch {
                match parse_line(&line) {
                    SseLine::Done => break 'outer,
                    SseLine::Skip => {}
                    SseLine::Data(p) => {
                        let Ok(ev) = serde_json::from_str::<Value>(&p) else {
                            continue;
                        };
                        if let Some(tok) = handle_event(LABEL, &ev, &mut state)? {
                            on_token(tok);
                        }
                        if state.finished {
                            break 'outer;
                        }
                    }
                }
            }
            if ended {
                break;
            }
        }
        if !state.finished {
            return Err(ApiError {
                message: "The response stream ended unexpectedly.".into(),
                status: None,
            }
            .into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn events_yield_text_and_track_completion() {
        let mut st = EventState::default();
        assert_eq!(
            handle_event(
                "A",
                &ev(r#"{"type":"message_start","message":{}}"#),
                &mut st
            )
            .unwrap(),
            None
        );
        assert_eq!(handle_event("A", &ev(r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#), &mut st).unwrap().as_deref(), Some("Hi"));
        assert_eq!(handle_event("A", &ev(r#"{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{"}}"#), &mut st).unwrap(), None);
        assert_eq!(
            handle_event(
                "A",
                &ev(r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":""}}"#),
                &mut st
            )
            .unwrap(),
            None
        );
        assert!(!st.finished);
        handle_event(
            "A",
            &ev(r#"{"type":"message_delta","delta":{"stop_reason":null}}"#),
            &mut st,
        )
        .unwrap();
        assert!(!st.finished);
        handle_event(
            "A",
            &ev(r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#),
            &mut st,
        )
        .unwrap();
        assert!(st.finished);
        let mut st = EventState::default();
        handle_event("A", &ev(r#"{"type":"message_stop"}"#), &mut st).unwrap();
        assert!(st.finished);
        let e = handle_event(
            "Anthropic",
            &ev(r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#),
            &mut st,
        )
        .unwrap_err();
        assert_eq!(e.message, "Anthropic stream error: Overloaded");
    }

    #[test]
    fn messages_are_normalized_for_the_api() {
        let m = |r: &str, c: &str| ChatMessage::new(r, c);
        // Leading assistant dropped, same-role runs merged, empty and error messages skipped.
        let out = normalize_messages(
            &[
                m("assistant", "stray"),
                m("user", "q1"),
                m("user", "q1b"),
                m("error", "boom"),
                m("assistant", "a1"),
                m("assistant", "  "),
            ],
            "q2",
        );
        let shape: Vec<_> = out
            .iter()
            .map(|m| (m.role.as_str(), m.content.as_str()))
            .collect();
        assert_eq!(
            shape,
            [("user", "q1\n\nq1b"), ("assistant", "a1"), ("user", "q2")]
        );
        // A previous turn left unanswered merges into the new question.
        let out = normalize_messages(&[m("user", "unanswered")], "next");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content, "unanswered\n\nnext");
        assert_eq!(normalize_messages(&[], "only")[0].content, "only");
    }

    #[test]
    fn error_text_prefers_the_api_message() {
        assert_eq!(error_text(br#"{"type":"error","error":{"type":"invalid_request_error","message":"bad model"}}"#), "bad model");
        assert_eq!(error_text(b"plain failure"), "plain failure");
    }

    #[test]
    fn debug_hides_the_key() {
        assert!(!format!("{:?}", Client::new("sk-ant-secret")).contains("sk-ant-secret"));
    }
}
