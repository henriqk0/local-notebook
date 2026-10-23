//! Plain data types shared across layers.

#[derive(Debug, Clone, Default)]
pub struct Notebook {
    pub id: i64,
    pub name: String,
    pub description: String,
    /// `None` means "use the default model".
    pub model: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct Source {
    pub id: i64,
    pub notebook_id: i64,
    pub path: String,
    pub filename: String,
    pub mime: Option<String>,
    pub ext: Option<String>,
    pub size: u64,
    pub content_text: String,
    pub token_estimate: usize,
    pub enabled: bool,
    /// `ok` or `error`.
    pub status: String,
    pub error: Option<String>,
    pub added_at: String,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            id: 0,
            notebook_id: 0,
            path: String::new(),
            filename: String::new(),
            mime: None,
            ext: None,
            size: 0,
            content_text: String::new(),
            token_estimate: 0,
            enabled: true,
            status: "ok".into(),
            error: None,
            added_at: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Message {
    pub id: i64,
    pub notebook_id: i64,
    /// `user`, `assistant` or `error`.
    pub role: String,
    pub content: String,
    pub model: Option<String>,
    pub created_at: String,
}

impl Message {
    pub fn new(notebook_id: i64, role: &str, content: impl Into<String>) -> Self {
        Self {
            notebook_id,
            role: role.into(),
            content: content.into(),
            ..Default::default()
        }
    }
}
