//! On-disk cache of the OpenRouter model list (24h TTL).
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Provider id (`openrouter`, `anthropic`, ...). Cache files from before
    /// multi-provider support have none and are OpenRouter's.
    #[serde(default = "default_provider")]
    pub provider: String,
    pub id: String,
    pub name: String,
    pub context_length: u64,
    pub prompt_price: f64,
    pub completion_price: f64,
    pub is_free: bool,
    /// Whether the provider reports per-token prices at all (Anthropic's model list doesn't).
    #[serde(default = "yes")]
    pub priced: bool,
}

fn default_provider() -> String {
    crate::llm::provider::OPENROUTER.to_string()
}

fn yes() -> bool {
    true
}

/// Numbers arrive as JSON numbers or numeric strings (OpenRouter prices are strings).
fn as_f64(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|f| f.is_finite())
    .unwrap_or(0.0)
}

fn as_u64(v: Option<&Value>) -> u64 {
    as_f64(v).max(0.0) as u64
}

fn string(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(String::from)
}

impl ModelInfo {
    /// Build from an API entry or a cache entry, coercing any malformed values.
    pub fn from_value(data: &Value) -> Self {
        let pricing = data.get("pricing");
        let has_pricing =
            pricing.is_some_and(Value::is_object) || data.get("prompt_price").is_some();
        let (prompt, completion) = match pricing {
            Some(p) if p.is_object() => (as_f64(p.get("prompt")), as_f64(p.get("completion"))),
            _ => (
                as_f64(data.get("prompt_price")),
                as_f64(data.get("completion_price")),
            ),
        };
        let priced = match data.get("priced") {
            Some(Value::Bool(b)) => *b,
            _ => has_pricing,
        };
        let id = string(data.get("id")).unwrap_or_default();
        let name = string(data.get("name"))
            .or_else(|| string(data.get("display_name")))
            .unwrap_or_else(|| id.clone());
        let is_free = match data.get("is_free") {
            Some(Value::Bool(b)) => *b && priced,
            _ => priced && prompt == 0.0 && completion == 0.0,
        };
        let context = data
            .get("context_length")
            .or_else(|| data.get("max_input_tokens"));
        Self {
            provider: string(data.get("provider"))
                .filter(|p| !p.is_empty())
                .unwrap_or_else(default_provider),
            id,
            name,
            context_length: as_u64(context),
            prompt_price: prompt,
            completion_price: completion,
            is_free,
            priced,
        }
    }

    /// Same entry, attributed to `provider`.
    pub fn for_provider(mut self, provider: &str) -> Self {
        self.provider = provider.to_string();
        self
    }

    /// The string stored in the config / notebooks to select this model.
    pub fn reference(&self) -> String {
        crate::llm::provider::ModelRef {
            provider: self.provider.clone(),
            model: self.id.clone(),
        }
        .to_string()
    }
}

pub struct ModelCache {
    path: PathBuf,
    ttl_seconds: f64,
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl ModelCache {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            ttl_seconds: 24.0 * 3600.0,
        }
    }

    /// Cached models, or empty when the file is missing, corrupt or stale.
    pub fn load(&self) -> Vec<ModelInfo> {
        let Some(Value::Object(root)) = fs::read_to_string(&self.path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
        else {
            return Vec::new();
        };
        if now() - as_f64(root.get("fetched_at")) > self.ttl_seconds {
            return Vec::new();
        }
        root.get("models")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter(|v| v.is_object())
                    .map(ModelInfo::from_value)
                    .filter(|m| !m.id.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn save(&self, models: &[ModelInfo]) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            crate::config::ensure_private_dir(dir)?;
        }
        let payload = serde_json::json!({ "fetched_at": now(), "models": models });
        let tmp = self.path.with_extension("json.tmp");
        let result = fs::write(&tmp, serde_json::to_vec(&payload)?)
            .and_then(|_| fs::rename(&tmp, &self.path));
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        Ok(result?)
    }

    /// Case-insensitive match of `query` against provider, id and name.
    pub fn matches(m: &ModelInfo, query: &str) -> bool {
        let q = query.to_lowercase();
        q.is_empty()
            || m.id.to_lowercase().contains(&q)
            || m.name.to_lowercase().contains(&q)
            || m.provider.to_lowercase().contains(&q)
    }

    pub fn filter<'a>(models: &'a [ModelInfo], query: &str) -> Vec<&'a ModelInfo> {
        models.iter().filter(|m| Self::matches(m, query)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn from_api_coerces_prices_and_defaults() {
        let m = ModelInfo::from_value(
            &json!({"id":"a/b","name":"AB","context_length":"8192","pricing":{"prompt":"0.000001","completion":0.000002}}),
        );
        assert_eq!(
            (
                m.context_length,
                m.prompt_price,
                m.completion_price,
                m.is_free
            ),
            (8192, 0.000001, 0.000002, false)
        );
        let free =
            ModelInfo::from_value(&json!({"id":"x","pricing":{"prompt":"0","completion":"0"}}));
        assert!(free.is_free && free.name == "x");
        let junk = ModelInfo::from_value(&json!({"id":5,"pricing":"oops","context_length":-3}));
        assert_eq!((junk.id.as_str(), junk.context_length), ("", 0));
    }

    #[test]
    fn save_load_roundtrip_ttl_and_corruption() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("models_cache.json");
        let cache = ModelCache::new(path.clone());
        assert!(cache.load().is_empty());
        let models = vec![ModelInfo::from_value(
            &json!({"id":"a/b","name":"AB","context_length":10,"pricing":{"prompt":"1","completion":"2"}}),
        )];
        cache.save(&models).unwrap();
        assert_eq!(cache.load(), models);

        fs::write(&path, r#"{"fetched_at": 0, "models": []}"#).unwrap();
        assert!(cache.load().is_empty(), "stale");
        fs::write(&path, "[1,2,3]").unwrap();
        assert!(cache.load().is_empty(), "wrong root type");
        fs::write(
            &path,
            format!(
                r#"{{"fetched_at": {}, "models": [1, {{"id":"ok"}}, "x"]}}"#,
                now()
            ),
        )
        .unwrap();
        assert_eq!(cache.load().len(), 1);
    }

    #[test]
    fn filter_matches_id_or_name_case_insensitively() {
        let a = ModelInfo::from_value(&json!({"id":"openai/gpt","name":"GPT"}));
        let b = ModelInfo::from_value(&json!({"id":"x/y","name":"Llama"}));
        let all = [a, b];
        assert_eq!(ModelCache::filter(&all, "LLAMA").len(), 1);
        assert_eq!(ModelCache::filter(&all, "openai")[0].id, "openai/gpt");
        assert_eq!(ModelCache::filter(&all, "").len(), 2);
    }
}
