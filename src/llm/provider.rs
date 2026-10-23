//! Provider abstraction: which backend serves a model, how to list its models and how
//! to stream a chat turn from it.
use std::fmt;

use tokio_util::sync::CancellationToken;

use super::models_cache::ModelInfo;
use super::openrouter::{self, ApiError, ChatMessage, StreamError};
use super::{anthropic, claude_cli};
use crate::sources::ranger;

pub const OPENROUTER: &str = "openrouter";
pub const ANTHROPIC: &str = "anthropic";
pub const CLAUDE_CLI: &str = "claude-cli";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    OpenRouter,
    /// Anthropic's Messages API with an API key.
    Anthropic,
    /// The official `claude` CLI, run headlessly with the user's Claude plan login.
    ClaudeCli,
    /// Any OpenAI-compatible `/chat/completions` endpoint (OpenAI, Ollama, LM Studio, ...).
    OpenAiCompatible,
}

impl ProviderKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "openrouter" => Some(Self::OpenRouter),
            "anthropic" => Some(Self::Anthropic),
            "claude-cli" | "claude-plan" | "claude" => Some(Self::ClaudeCli),
            "openai-compatible" | "openai" => Some(Self::OpenAiCompatible),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenRouter => "openrouter",
            Self::Anthropic => "anthropic",
            Self::ClaudeCli => "claude-cli",
            Self::OpenAiCompatible => "openai-compatible",
        }
    }
}

/// A model plus the provider that serves it. Stored as `provider::model`; a bare id
/// means OpenRouter (the original provider), so notebooks saved before multi-provider
/// support keep working.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
}

impl ModelRef {
    pub fn parse(s: &str) -> Self {
        match s.split_once("::") {
            Some((p, m)) if !p.is_empty() && !m.is_empty() => Self {
                provider: p.to_string(),
                model: m.to_string(),
            },
            _ => Self {
                provider: OPENROUTER.to_string(),
                model: s.to_string(),
            },
        }
    }

    /// For the UI: `model` for OpenRouter, `provider · model` otherwise.
    pub fn short(&self) -> String {
        if self.provider == OPENROUTER {
            self.model.clone()
        } else {
            format!("{} · {}", self.provider, self.model)
        }
    }
}

impl fmt::Display for ModelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.provider == OPENROUTER {
            f.write_str(&self.model)
        } else {
            write!(f, "{}::{}", self.provider, self.model)
        }
    }
}

#[derive(Clone)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    /// Overrides the default endpoint (required for `OpenAiCompatible`).
    pub base_url: Option<String>,
    /// The key in use: the environment wins over the saved one.
    pub api_key: Option<String>,
    /// The only key that may be written to disk; environment keys never land here.
    pub saved_key: Option<String>,
    /// Environment variable consulted for the key.
    pub api_key_env: Option<String>,
    /// The CLI to run (`ClaudeCli` only), possibly with leading arguments.
    pub command: String,
    pub builtin: bool,
}

// Keys must never reach logs or panic messages.
impl fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("base_url", &self.base_url)
            .field("has_key", &self.api_key.is_some())
            .finish_non_exhaustive()
    }
}

impl ProviderConfig {
    pub fn builtin(kind: ProviderKind) -> Self {
        let (id, name, env) = match kind {
            ProviderKind::OpenRouter => (OPENROUTER, "OpenRouter", Some("OPENROUTER_API_KEY")),
            ProviderKind::Anthropic => (ANTHROPIC, "Anthropic API", Some("ANTHROPIC_API_KEY")),
            ProviderKind::ClaudeCli => (CLAUDE_CLI, "Claude plan (claude CLI)", None),
            ProviderKind::OpenAiCompatible => ("openai-compatible", "OpenAI-compatible", None),
        };
        Self {
            id: id.into(),
            name: name.into(),
            kind,
            base_url: None,
            api_key: None,
            saved_key: None,
            api_key_env: env.map(String::from),
            command: "claude".into(),
            builtin: true,
        }
    }

    /// Point at a different endpoint (tests, proxies).
    pub fn with_base_url(mut self, url: &str) -> Self {
        self.base_url = Some(url.to_string());
        self
    }

    pub fn with_key(mut self, key: &str) -> Self {
        self.api_key = Some(key.to_string());
        self
    }

    /// Whether this provider takes an API key at all.
    pub fn uses_key(&self) -> bool {
        self.kind != ProviderKind::ClaudeCli
    }

    /// Whether the key is mandatory (local OpenAI-compatible servers usually need none).
    pub fn key_required(&self) -> bool {
        matches!(
            self.kind,
            ProviderKind::OpenRouter | ProviderKind::Anthropic
        )
    }

    pub fn is_ready(&self) -> bool {
        match self.kind {
            ProviderKind::OpenRouter | ProviderKind::Anthropic => self.api_key.is_some(),
            ProviderKind::ClaudeCli => ranger::is_available(&self.command),
            ProviderKind::OpenAiCompatible => self.base_url.is_some(),
        }
    }

    /// One-line state for the providers dialog.
    pub fn status(&self) -> String {
        match self.kind {
            ProviderKind::ClaudeCli if self.is_ready() => {
                "ready (uses your Claude plan login)".into()
            }
            ProviderKind::ClaudeCli => format!(
                "`{}` not found in PATH",
                self.command.split_whitespace().next().unwrap_or("claude")
            ),
            ProviderKind::OpenAiCompatible if self.base_url.is_none() => {
                "no base_url configured".into()
            }
            _ if self.api_key.is_some() && self.api_key != self.saved_key => {
                "ready (key from environment)".into()
            }
            _ if self.api_key.is_some() => "ready".into(),
            ProviderKind::OpenAiCompatible => "ready (no key)".into(),
            _ => "needs an API key".into(),
        }
    }

    fn missing_key(&self) -> ApiError {
        ApiError {
            message: format!("{} needs an API key. Press Ctrl+K to add one.", self.name),
            status: None,
        }
    }
}

/// Everything one chat turn sends, before a provider decides how to lay it out.
#[derive(Debug, Clone, Default)]
pub struct Prompt {
    pub system: String,
    /// The rendered `=== SOURCE ... ===` blocks; empty when there are none.
    pub sources: String,
    /// Earlier user/assistant messages, oldest first.
    pub history: Vec<ChatMessage>,
    pub question: String,
}

impl Prompt {
    /// The system text for providers that take sources in the system message.
    pub fn system_with_sources(&self) -> String {
        if self.sources.is_empty() {
            self.system.clone()
        } else {
            format!("{}\n\n{}", self.system, self.sources)
        }
    }

    /// system + history + question, OpenAI style.
    pub fn openai_messages(&self) -> Vec<ChatMessage> {
        let mut v = vec![ChatMessage::new("system", self.system_with_sources())];
        v.extend(self.history.iter().cloned());
        v.push(ChatMessage::new("user", self.question.clone()));
        v
    }
}

/// The models `provider` offers right now (a network call for the API providers).
pub async fn list_models(p: &ProviderConfig) -> Result<Vec<ModelInfo>, ApiError> {
    let raw = match p.kind {
        ProviderKind::ClaudeCli => return Ok(claude_cli::models()),
        ProviderKind::OpenRouter => {
            let key = p.api_key.as_deref().ok_or_else(|| p.missing_key())?;
            let client = match &p.base_url {
                Some(url) => openrouter::Client::with_base_url(key, url),
                None => openrouter::Client::new(key),
            };
            client.list_models().await?
        }
        ProviderKind::OpenAiCompatible => compatible_client(p)?.list_models().await?,
        ProviderKind::Anthropic => anthropic_client(p)?.list_models().await?,
    };
    Ok(raw
        .iter()
        .map(ModelInfo::from_value)
        .filter(|m| !m.id.is_empty())
        .map(|m| m.for_provider(&p.id))
        .collect())
}

fn compatible_client(p: &ProviderConfig) -> Result<openrouter::Client, ApiError> {
    let url = p.base_url.as_deref().ok_or_else(|| ApiError {
        message: format!("{} has no base_url configured.", p.name),
        status: None,
    })?;
    Ok(openrouter::Client::compatible(
        &p.name,
        url,
        p.api_key.as_deref(),
    ))
}

fn anthropic_client(p: &ProviderConfig) -> Result<anthropic::Client, ApiError> {
    let key = p.api_key.as_deref().ok_or_else(|| p.missing_key())?;
    Ok(match &p.base_url {
        Some(url) => anthropic::Client::with_base_url(key, url),
        None => anthropic::Client::new(key),
    })
}

/// Stream one chat turn from `p`, calling `on_token` for each piece of text.
pub async fn stream_chat(
    p: &ProviderConfig,
    model: &str,
    prompt: &Prompt,
    cancel: &CancellationToken,
    on_token: impl FnMut(String),
) -> Result<(), StreamError> {
    match p.kind {
        ProviderKind::OpenRouter => {
            let key = p.api_key.as_deref().ok_or_else(|| p.missing_key())?;
            let client = match &p.base_url {
                Some(url) => openrouter::Client::with_base_url(key, url),
                None => openrouter::Client::new(key),
            };
            client
                .stream_chat(model, &prompt.openai_messages(), cancel, on_token)
                .await
        }
        ProviderKind::OpenAiCompatible => {
            compatible_client(p)?
                .stream_chat(model, &prompt.openai_messages(), cancel, on_token)
                .await
        }
        ProviderKind::Anthropic => {
            anthropic_client(p)?
                .stream_chat(model, prompt, cancel, on_token)
                .await
        }
        ProviderKind::ClaudeCli => {
            claude_cli::stream_chat(&p.command, model, prompt, cancel, on_token).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_refs_round_trip_and_bare_ids_are_openrouter() {
        let r = ModelRef::parse("openai/gpt-4o");
        assert_eq!(
            (r.provider.as_str(), r.model.as_str()),
            ("openrouter", "openai/gpt-4o")
        );
        assert_eq!(
            r.to_string(),
            "openai/gpt-4o",
            "OpenRouter ids stay bare so notebooks saved before multi-provider support keep working"
        );
        let r = ModelRef::parse("meta-llama/llama-3.3-70b-instruct:free");
        assert_eq!(r.provider, "openrouter", "a single colon is part of the id");

        let r = ModelRef::parse("anthropic::claude-sonnet-5-5");
        assert_eq!(
            (r.provider.as_str(), r.model.as_str()),
            ("anthropic", "claude-sonnet-5-5")
        );
        assert_eq!(r.to_string(), "anthropic::claude-sonnet-5-5");
        assert_eq!(r.short(), "anthropic · claude-sonnet-5-5");
        assert_eq!(ModelRef::parse("local::llama3:8b").model, "llama3:8b");
        assert_eq!(ModelRef::parse("::x").provider, "openrouter");
        assert_eq!(ModelRef::parse("x::").model, "x::");
    }

    #[test]
    fn kinds_parse_with_aliases() {
        assert_eq!(
            ProviderKind::parse("Claude-Plan"),
            Some(ProviderKind::ClaudeCli)
        );
        assert_eq!(
            ProviderKind::parse("openai"),
            Some(ProviderKind::OpenAiCompatible)
        );
        assert_eq!(ProviderKind::parse("nope"), None);
        for k in [
            ProviderKind::OpenRouter,
            ProviderKind::Anthropic,
            ProviderKind::ClaudeCli,
            ProviderKind::OpenAiCompatible,
        ] {
            assert_eq!(ProviderKind::parse(k.as_str()), Some(k));
        }
    }

    #[test]
    fn readiness_and_status() {
        let mut or = ProviderConfig::builtin(ProviderKind::OpenRouter);
        assert!(!or.is_ready() && or.status() == "needs an API key");
        or.api_key = Some("k".into());
        assert!(or.is_ready() && or.status() == "ready (key from environment)");
        or.saved_key = Some("k".into());
        assert_eq!(or.status(), "ready");

        let mut local = ProviderConfig::builtin(ProviderKind::OpenAiCompatible);
        assert!(!local.is_ready());
        local.base_url = Some("http://localhost:11434/v1".into());
        assert!(local.is_ready() && !local.key_required() && local.status() == "ready (no key)");

        let mut cli = ProviderConfig::builtin(ProviderKind::ClaudeCli);
        cli.command = "definitely-not-installed-xyz".into();
        assert!(!cli.is_ready() && cli.status().contains("not found"));
        cli.command = "sh".into();
        assert!(cli.is_ready());
        assert!(!cli.uses_key());
    }

    #[test]
    fn debug_never_prints_the_key() {
        let p = ProviderConfig::builtin(ProviderKind::Anthropic).with_key("sk-ant-secret");
        assert!(!format!("{p:?}").contains("sk-ant-secret"));
    }

    #[test]
    fn prompt_layouts() {
        let p = Prompt {
            system: "SYS".into(),
            sources: "=== SOURCE: a ===".into(),
            history: vec![
                ChatMessage::new("user", "q1"),
                ChatMessage::new("assistant", "a1"),
            ],
            question: "q2".into(),
        };
        assert_eq!(p.system_with_sources(), "SYS\n\n=== SOURCE: a ===");
        let m = p.openai_messages();
        assert_eq!(
            m.iter().map(|m| m.role.as_str()).collect::<Vec<_>>(),
            ["system", "user", "assistant", "user"]
        );
        assert_eq!(
            Prompt {
                system: "S".into(),
                ..Default::default()
            }
            .system_with_sources(),
            "S"
        );
    }
}
