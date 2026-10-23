//! rsnotebook: a terminal NotebookLM-style app. Domain logic and I/O live in
//! the modules below; `ui` and `app` are the only parts that touch the terminal.
pub mod app;
pub mod chat;
pub mod config;
pub mod llm;
pub mod models;
pub mod sources;
pub mod store;
pub mod tokens;
pub mod ui;
