//! One chat turn: build the prompt, stream the answer, persist the outcome.
//! No terminal code here, so the whole flow is testable against a fake server.
use anyhow::{Result, anyhow};
use tokio_util::sync::CancellationToken;

use crate::llm::context::{self, ChunkHit};
use crate::llm::openrouter::{ChatMessage, StreamError};
use crate::llm::provider::{self, ModelRef, Prompt, ProviderConfig};
use crate::models::Message;
use crate::store::Store;

pub const ANSWER_RESERVE_TOKENS: usize = 1024;
const MAX_RETRIEVED_CHUNKS: usize = 20;

pub struct GenJob {
    pub store: Store,
    pub notebook_id: i64,
    pub question: String,
    /// Messages before this question (the question itself is added separately).
    pub prior: Vec<Message>,
    /// A model reference (`provider::model`, or a bare OpenRouter id).
    pub model: Option<String>,
    /// The configured providers; the model's provider is looked up here.
    pub providers: Vec<ProviderConfig>,
    pub system_prompt: String,
    pub token_budget: usize,
    pub cancel: CancellationToken,
}

#[derive(Debug)]
pub enum GenEvent {
    Token(String),
    Done(GenOutcome),
}

/// What to show once the turn is over.
#[derive(Debug, Default)]
pub struct GenOutcome {
    /// Messages that were written to the database, in order.
    pub saved: Vec<Message>,
    /// An error to display that is *not* stored (a partial answer was stored instead).
    pub transient_error: Option<String>,
}

/// Assemble the prompt for a question: system text, rendered sources, trimmed history.
pub fn build_prompt(
    store: &Store,
    notebook_id: i64,
    question: &str,
    prior: &[Message],
    system_prompt: &str,
    token_budget: usize,
) -> Result<Prompt> {
    let sources = store.list_sources_for_context(notebook_id)?;
    let mut search_error: Option<anyhow::Error> = None;
    let ctx = context::build_context(
        question,
        &sources,
        prior,
        system_prompt,
        token_budget,
        |q| {
            let query = context::sanitize_fts_query(q);
            match store.search_chunks(notebook_id, &query, MAX_RETRIEVED_CHUNKS) {
                Ok(rows) => rows
                    .into_iter()
                    .map(|(source_id, filename, text)| ChunkHit {
                        source_id,
                        filename,
                        text,
                    })
                    .collect(),
                Err(e) => {
                    search_error = Some(e);
                    Vec::new()
                }
            }
        },
        ANSWER_RESERVE_TOKENS,
    );
    if let Some(e) = search_error {
        return Err(e);
    }

    Ok(Prompt {
        system: ctx.system_prompt,
        sources: ctx.sources_block,
        history: ctx
            .history
            .iter()
            .filter(|m| m.role == "user" || m.role == "assistant")
            .map(|m| ChatMessage::new(&m.role, m.content.clone()))
            .collect(),
        question: question.to_string(),
    })
}

/// Store a message; if the database refuses, still return it so it is displayed.
fn persist(
    store: &Store,
    notebook_id: i64,
    role: &str,
    content: String,
    model: Option<&str>,
) -> Message {
    let mut msg = Message::new(notebook_id, role, content);
    msg.model = model.map(String::from);
    store.add_message(&msg).unwrap_or(msg)
}

pub async fn run(job: GenJob, emit: impl Fn(GenEvent) + Send + Sync) {
    let outcome = run_inner(&job, &emit).await;
    emit(GenEvent::Done(outcome));
}

async fn run_inner(job: &GenJob, emit: &(impl Fn(GenEvent) + Send + Sync)) -> GenOutcome {
    let fail = |text: &str| GenOutcome {
        saved: vec![persist(
            &job.store,
            job.notebook_id,
            "error",
            text.to_string(),
            None,
        )],
        transient_error: None,
    };

    let Some(model) = job.model.clone() else {
        return fail("No model selected. Press Ctrl+O to choose one.");
    };
    let model_ref = ModelRef::parse(&model);
    let Some(provider) = job.providers.iter().find(|p| p.id == model_ref.provider) else {
        return fail(&format!(
            "Unknown provider '{}'. Press Ctrl+K to see the available ones.",
            model_ref.provider
        ));
    };

    let built = {
        let (store, question, prior, prompt) = (
            job.store.clone(),
            job.question.clone(),
            job.prior.clone(),
            job.system_prompt.clone(),
        );
        let (nb, budget) = (job.notebook_id, job.token_budget);
        tokio::task::spawn_blocking(move || {
            build_prompt(&store, nb, &question, &prior, &prompt, budget)
        })
        .await
        .unwrap_or_else(|e| Err(anyhow!("{e}")))
    };
    let prompt = match built {
        Ok(p) => p,
        Err(e) => return fail(&format!("Could not build context: {e}")),
    };

    let mut parts = String::new();
    let result = provider::stream_chat(provider, &model_ref.model, &prompt, &job.cancel, |token| {
        parts.push_str(&token);
        emit(GenEvent::Token(token));
    })
    .await;

    let store = &job.store;
    let nb = job.notebook_id;
    match result {
        Ok(()) if parts.is_empty() => fail("The model returned an empty response."),
        Ok(()) => GenOutcome {
            saved: vec![persist(store, nb, "assistant", parts, Some(&model))],
            transient_error: None,
        },
        Err(StreamError::Cancelled) if parts.is_empty() => GenOutcome::default(),
        Err(StreamError::Cancelled) => {
            parts.push_str("\n\n[interrupted]");
            GenOutcome {
                saved: vec![persist(store, nb, "assistant", parts, Some(&model))],
                transient_error: None,
            }
        }
        Err(StreamError::Api(e)) if parts.is_empty() => fail(&e.message),
        Err(StreamError::Api(e)) => {
            parts.push_str("\n\n[interrupted: error]");
            GenOutcome {
                saved: vec![persist(store, nb, "assistant", parts, Some(&model))],
                transient_error: Some(e.message),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::ProviderKind;
    use crate::models::Source;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Serve one canned SSE response; returns the base URL and the request body seen.
    fn fake_server(
        chunks: Vec<&'static str>,
        then_stall: bool,
    ) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut req = Vec::new();
            let mut b = [0u8; 1];
            while !req.ends_with(b"\r\n\r\n") {
                sock.read_exact(&mut b).unwrap();
                req.push(b[0]);
            }
            let head = String::from_utf8_lossy(&req).to_lowercase();
            let len: usize = head
                .lines()
                .find_map(|l| {
                    l.strip_prefix("content-length:")
                        .map(|v| v.trim().parse().unwrap())
                })
                .unwrap_or(0);
            let mut body = vec![0u8; len];
            sock.read_exact(&mut body).unwrap();
            let _ = tx.send(String::from_utf8_lossy(&body).to_string());
            sock.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
            )
            .unwrap();
            for c in chunks {
                sock.write_all(c.as_bytes()).unwrap();
                sock.flush().unwrap();
                std::thread::sleep(Duration::from_millis(10));
            }
            if then_stall {
                std::thread::sleep(Duration::from_secs(5));
            }
        });
        (url, rx)
    }

    fn job(store: &Store, nb: i64, base_url: Option<String>) -> GenJob {
        let mut openrouter = ProviderConfig::builtin(ProviderKind::OpenRouter).with_key("k");
        openrouter.base_url = base_url;
        GenJob {
            store: store.clone(),
            notebook_id: nb,
            question: "What is in the doc?".into(),
            prior: vec![],
            model: Some("m/x".into()),
            providers: vec![openrouter],
            system_prompt: "SYS".into(),
            token_budget: 24_000,
            cancel: CancellationToken::new(),
        }
    }

    fn setup() -> (Store, i64) {
        let store = Store::open_in_memory().unwrap();
        let nb = store.create_notebook("N", "").unwrap();
        let src = Source {
            notebook_id: nb,
            filename: "doc.txt".into(),
            content_text: "The sky is blue.".into(),
            path: "/doc.txt".into(),
            ..Default::default()
        };
        store
            .add_source_with_chunks(&src, &["The sky is blue.".into()])
            .unwrap();
        (store, nb)
    }

    async fn run_collect(job: GenJob) -> (String, GenOutcome) {
        let tokens = Mutex::new(String::new());
        let outcome = Mutex::new(None);
        run(job, |ev| match ev {
            GenEvent::Token(t) => tokens.lock().unwrap().push_str(&t),
            GenEvent::Done(o) => *outcome.lock().unwrap() = Some(o),
        })
        .await;
        (
            tokens.into_inner().unwrap(),
            outcome
                .into_inner()
                .unwrap()
                .expect("Done is always emitted"),
        )
    }

    #[test]
    fn the_prompt_keeps_sources_apart_and_drops_non_chat_rows() {
        let (store, nb) = setup();
        let mut j = job(&store, nb, None);
        j.prior = vec![
            Message::new(nb, "user", "earlier"),
            Message::new(nb, "error", "boom"),
            Message::new(nb, "assistant", "answer"),
        ];
        let p = build_prompt(
            &store,
            nb,
            &j.question,
            &j.prior,
            &j.system_prompt,
            j.token_budget,
        )
        .unwrap();
        assert_eq!(p.system, "SYS");
        assert!(
            p.sources.starts_with("=== SOURCE: doc.txt ===")
                && p.sources.contains("The sky is blue.")
        );
        assert_eq!(p.question, "What is in the doc?");
        let roles: Vec<_> = p.history.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, ["user", "assistant"]);

        // OpenAI-style providers get it as system(+sources), history, question.
        let m = p.openai_messages();
        assert!(m[0].content.starts_with("SYS\n\n=== SOURCE: doc.txt ==="));
        assert_eq!(m.last().unwrap().content, "What is in the doc?");
    }

    #[tokio::test]
    async fn successful_turn_streams_and_saves_the_answer() {
        let (store, nb) = setup();
        let (url, body) = fake_server(
            vec![
                "data: {\"choices\":[{\"delta\":{\"content\":\"Blue\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\".\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
            ],
            false,
        );
        let (tokens, out) = run_collect(job(&store, nb, Some(url))).await;
        assert_eq!(tokens, "Blue.");
        assert_eq!(out.saved.len(), 1);
        assert_eq!(
            (
                out.saved[0].role.as_str(),
                out.saved[0].content.as_str(),
                out.saved[0].model.as_deref()
            ),
            ("assistant", "Blue.", Some("m/x"))
        );
        assert!(body.recv().unwrap().contains("The sky is blue."));
        assert_eq!(store.list_messages(nb, None).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn missing_model_or_key_is_recorded_as_an_error_message() {
        let (store, nb) = setup();
        let mut j = job(&store, nb, None);
        j.model = None;
        let (_, out) = run_collect(j).await;
        assert!(out.saved[0].role == "error" && out.saved[0].content.contains("No model selected"));

        let mut j = job(&store, nb, None);
        j.providers[0].api_key = None;
        let (_, out) = run_collect(j).await;
        assert!(out.saved[0].content.contains("needs an API key"));
        assert_eq!(store.list_messages(nb, None).unwrap().len(), 2);

        let mut j = job(&store, nb, None);
        j.model = Some("nowhere::some-model".into());
        let (_, out) = run_collect(j).await;
        assert!(out.saved[0].content.contains("Unknown provider 'nowhere'"));
    }

    #[tokio::test]
    async fn a_prefixed_model_goes_to_its_own_provider_and_is_stored_with_the_prefix() {
        let (store, nb) = setup();
        let (url, body) = fake_server(
            vec![
                "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Blue.\"}}\n\ndata: {\"type\":\"message_stop\"}\n\n",
            ],
            false,
        );
        let mut j = job(&store, nb, None);
        j.providers.push(
            ProviderConfig::builtin(ProviderKind::Anthropic)
                .with_key("sk-ant")
                .with_base_url(&url),
        );
        j.model = Some("anthropic::claude-x".into());
        let (tokens, out) = run_collect(j).await;
        assert_eq!(tokens, "Blue.");
        assert_eq!(out.saved[0].model.as_deref(), Some("anthropic::claude-x"));
        let sent = body.recv().unwrap();
        assert!(
            sent.contains("\"model\":\"claude-x\"") && sent.contains("\"max_tokens\""),
            "{sent}"
        );
    }

    #[tokio::test]
    async fn truncated_stream_keeps_the_partial_answer_and_reports_the_error() {
        let (store, nb) = setup();
        let (url, _) = fake_server(
            vec!["data: {\"choices\":[{\"delta\":{\"content\":\"Half an ans\"}}]}\n\n"],
            false,
        );
        let (_, out) = run_collect(job(&store, nb, Some(url))).await;
        assert_eq!(out.saved.len(), 1);
        assert_eq!(out.saved[0].content, "Half an ans\n\n[interrupted: error]");
        assert!(out.transient_error.unwrap().contains("ended unexpectedly"));
    }

    #[tokio::test]
    async fn empty_response_is_an_error() {
        let (store, nb) = setup();
        let (url, _) = fake_server(
            vec![
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
            ],
            false,
        );
        let (_, out) = run_collect(job(&store, nb, Some(url))).await;
        assert!(out.saved[0].role == "error" && out.saved[0].content.contains("empty response"));
    }

    #[tokio::test]
    async fn cancel_saves_the_partial_answer() {
        let (store, nb) = setup();
        let (url, _) = fake_server(
            vec!["data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n"],
            true,
        );
        let j = job(&store, nb, Some(url));
        let cancel = j.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            cancel.cancel();
        });
        let (tokens, out) = run_collect(j).await;
        assert_eq!(tokens, "partial");
        assert_eq!(out.saved[0].content, "partial\n\n[interrupted]");
        assert!(out.transient_error.is_none());
    }

    #[tokio::test]
    async fn cancel_before_any_output_saves_nothing() {
        let (store, nb) = setup();
        let (url, _) = fake_server(vec![], true);
        let j = job(&store, nb, Some(url));
        j.cancel.cancel();
        let (_, out) = run_collect(j).await;
        assert!(out.saved.is_empty());
        assert!(store.list_messages(nb, None).unwrap().is_empty());
    }
}
