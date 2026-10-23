//! The Anthropic, OpenAI-compatible and Claude-CLI providers against fake backends.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use rsnotebook::llm::openrouter::{ChatMessage, StreamError};
use rsnotebook::llm::provider::{self, ANTHROPIC, Prompt, ProviderConfig, ProviderKind};
use tokio_util::sync::CancellationToken;

fn serve(
    respond: impl FnOnce(&mut TcpStream) + Send + 'static,
) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut req = Vec::new();
        let mut b = [0u8; 1];
        while !req.ends_with(b"\r\n\r\n") {
            sock.read_exact(&mut b).unwrap();
            req.push(b[0]);
        }
        let head = String::from_utf8_lossy(&req).to_string();
        let len = head
            .lines()
            .find_map(|l| {
                l.to_lowercase()
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        let mut body = vec![0u8; len];
        sock.read_exact(&mut body).unwrap();
        let _ = tx.send(head + &String::from_utf8_lossy(&body));
        respond(&mut sock);
    });
    (url, rx)
}

fn sse(sock: &mut TcpStream, chunks: &[&str]) {
    sock.write_all(
        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
    )
    .unwrap();
    for c in chunks {
        sock.write_all(c.as_bytes()).unwrap();
        sock.flush().unwrap();
        thread::sleep(Duration::from_millis(10));
    }
}

fn json_response(sock: &mut TcpStream, status: &str, body: &str) {
    sock.write_all(format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes()).unwrap();
}

fn prompt() -> Prompt {
    Prompt {
        system: "SYS".into(),
        sources: "=== SOURCE: doc.txt ===\nThe sky is blue.\n=== END SOURCE: doc.txt ===".into(),
        history: vec![
            ChatMessage::new("user", "earlier"),
            ChatMessage::new("assistant", "reply"),
            ChatMessage::new("error", "boom"),
        ],
        question: "Colour?".into(),
    }
}

async fn run(
    p: &ProviderConfig,
    model: &str,
    cancel: &CancellationToken,
) -> (Result<(), StreamError>, String) {
    let mut out = String::new();
    let r = provider::stream_chat(p, model, &prompt(), cancel, |t| out.push_str(&t)).await;
    (r, out)
}

fn api_error(r: Result<(), StreamError>) -> String {
    match r {
        Err(StreamError::Api(e)) => e.message,
        other => panic!("expected an API error, got {other:?}"),
    }
}

// ---- Anthropic API --------------------------------------------------------------

const ANTHROPIC_STREAM: &[&str] = &[
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Blue\"}}\n\nevent: ping\ndata: {\"type\":\"ping\"}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\".\"}}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
];

fn anthropic(url: &str) -> ProviderConfig {
    ProviderConfig::builtin(ProviderKind::Anthropic)
        .with_key("sk-ant-test")
        .with_base_url(url)
}

#[tokio::test]
async fn anthropic_streams_and_sends_the_expected_request() {
    let (url, rx) = serve(|s| sse(s, ANTHROPIC_STREAM));
    let (r, out) = run(&anthropic(&url), "claude-x", &CancellationToken::new()).await;
    r.unwrap();
    assert_eq!(out, "Blue.");

    let req = rx.recv().unwrap();
    let lower = req.to_lowercase();
    assert!(lower.starts_with("post /v1/messages "), "{req}");
    assert!(
        lower.contains("x-api-key: sk-ant-test") && lower.contains("anthropic-version: 2023-06-01")
    );
    assert!(
        !lower.contains("authorization:"),
        "no bearer token for Anthropic"
    );
    let body: serde_json::Value =
        serde_json::from_str(req.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["model"], "claude-x");
    assert_eq!(body["stream"], true);
    assert!(body["max_tokens"].as_u64().unwrap() > 0);
    let system = body["system"].as_str().unwrap();
    assert!(
        system.starts_with("SYS") && system.contains("The sky is blue."),
        "sources ride in the system prompt"
    );
    let roles: Vec<_> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(
        roles,
        ["user", "assistant", "user"],
        "error rows are dropped and roles alternate"
    );
}

#[tokio::test]
async fn anthropic_http_errors_are_friendly() {
    let (url, _) = serve(|s| {
        json_response(
            s,
            "401 Unauthorized",
            r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#,
        )
    });
    let msg = api_error(
        run(&anthropic(&url), "m", &CancellationToken::new())
            .await
            .0,
    );
    assert!(
        msg.contains("Invalid API key") && msg.contains("Anthropic"),
        "{msg}"
    );

    let (url, _) = serve(|s| {
        json_response(
            s,
            "400 Bad Request",
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"model: unknown"}}"#,
        )
    });
    assert_eq!(
        api_error(
            run(&anthropic(&url), "m", &CancellationToken::new())
                .await
                .0
        ),
        "Anthropic error 400: model: unknown"
    );
}

#[tokio::test]
async fn anthropic_truncation_error_events_and_cancel() {
    let (url, _) = serve(|s| sse(s, &ANTHROPIC_STREAM[..2]));
    let (r, out) = run(&anthropic(&url), "m", &CancellationToken::new()).await;
    assert_eq!(out, "Blue");
    assert!(api_error(r).contains("ended unexpectedly"));

    let (url, _) = serve(|s| {
        sse(
            s,
            &[
                "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n",
            ],
        )
    });
    assert!(
        api_error(
            run(&anthropic(&url), "m", &CancellationToken::new())
                .await
                .0
        )
        .contains("Overloaded")
    );

    let (url, _) = serve(|s| {
        sse(s, &ANTHROPIC_STREAM[..2]);
        thread::sleep(Duration::from_secs(5));
    });
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        c2.cancel();
    });
    let started = Instant::now();
    let (r, out) = run(&anthropic(&url), "m", &cancel).await;
    assert!(
        matches!(r, Err(StreamError::Cancelled))
            && out == "Blue"
            && started.elapsed() < Duration::from_secs(3)
    );
}

#[tokio::test]
async fn anthropic_model_list_is_attributed_and_unpriced() {
    let body = r#"{"data":[{"type":"model","id":"claude-sonnet-5-5","display_name":"Claude Sonnet 5.5","max_input_tokens":200000},{"id":"claude-haiku-5-5"}],"has_more":false}"#;
    let (url, rx) = serve(move |s| json_response(s, "200 OK", body));
    let models = provider::list_models(&anthropic(&url)).await.unwrap();
    assert!(rx.recv().unwrap().starts_with("GET /v1/models"));
    assert_eq!(models.len(), 2);
    assert_eq!(
        (
            models[0].provider.as_str(),
            models[0].name.as_str(),
            models[0].context_length,
            models[0].priced,
            models[0].is_free
        ),
        (ANTHROPIC, "Claude Sonnet 5.5", 200000, false, false)
    );
    assert_eq!(models[1].reference(), "anthropic::claude-haiku-5-5");
}

#[tokio::test]
async fn providers_without_a_key_explain_what_to_do() {
    let p = ProviderConfig::builtin(ProviderKind::Anthropic);
    let msg = api_error(run(&p, "m", &CancellationToken::new()).await.0);
    assert!(msg.contains("needs an API key") && msg.contains("Ctrl+K"));
    assert!(provider::list_models(&p).await.is_err());
}

// ---- OpenAI-compatible ----------------------------------------------------------

const OPENAI_STREAM: &[&str] = &[
    "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"!\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
];

fn compatible(url: &str, key: Option<&str>) -> ProviderConfig {
    let mut p = ProviderConfig::builtin(ProviderKind::OpenAiCompatible).with_base_url(url);
    p.id = "ollama".into();
    p.name = "Ollama".into();
    p.api_key = key.map(String::from);
    p
}

#[tokio::test]
async fn openai_compatible_works_without_a_key_and_without_openrouter_headers() {
    let (url, rx) = serve(|s| sse(s, OPENAI_STREAM));
    let (r, out) = run(
        &compatible(&url, None),
        "llama3:8b",
        &CancellationToken::new(),
    )
    .await;
    r.unwrap();
    assert_eq!(out, "Hi!");
    let req = rx.recv().unwrap().to_lowercase();
    assert!(req.starts_with("post /chat/completions "), "{req}");
    assert!(
        !req.contains("authorization:")
            && !req.contains("http-referer")
            && !req.contains("x-title")
    );
    assert!(req.contains("\"model\":\"llama3:8b\"") && req.contains("the sky is blue."));

    let (url, rx) = serve(|s| sse(s, OPENAI_STREAM));
    run(
        &compatible(&url, Some("local-key")),
        "m",
        &CancellationToken::new(),
    )
    .await
    .0
    .unwrap();
    assert!(
        rx.recv()
            .unwrap()
            .to_lowercase()
            .contains("authorization: bearer local-key")
    );
}

#[tokio::test]
async fn openai_compatible_errors_name_the_provider_and_models_are_tagged() {
    let (url, _) = serve(|s| json_response(s, "500 Internal Server Error", "model crashed"));
    assert!(
        api_error(
            run(&compatible(&url, None), "m", &CancellationToken::new())
                .await
                .0
        )
        .starts_with("Ollama error 500: model crashed")
    );

    let (url, _) = serve(|s| {
        json_response(
            s,
            "200 OK",
            r#"{"data":[{"id":"llama3:8b"},{"id":"qwen2"}]}"#,
        )
    });
    let models = provider::list_models(&compatible(&url, None))
        .await
        .unwrap();
    assert_eq!(
        models.iter().map(|m| m.reference()).collect::<Vec<_>>(),
        ["ollama::llama3:8b", "ollama::qwen2"]
    );
    assert!(models.iter().all(|m| !m.priced && !m.is_free));

    let mut no_url = ProviderConfig::builtin(ProviderKind::OpenAiCompatible);
    no_url.name = "Mine".into();
    assert!(
        api_error(run(&no_url, "m", &CancellationToken::new()).await.0).contains("no base_url")
    );
}

// ---- Claude plan via the CLI ----------------------------------------------------

/// A stand-in for `claude`: records its arguments, stdin and environment, then replies.
fn fake_claude(dir: &Path, body: &str) -> ProviderConfig {
    let script = dir.join("fake_claude.sh");
    std::fs::write(&script, format!("#!/bin/sh\nD=\"$(dirname \"$0\")\"\nprintf '%s\\n' \"$@\" > \"$D/args\"\ncat > \"$D/stdin\"\npwd > \"$D/cwd\"\nenv | grep -c '^ANTHROPIC_' > \"$D/anthropic_env\"\n{body}\n")).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o644)).unwrap();
    let mut p = ProviderConfig::builtin(ProviderKind::ClaudeCli);
    // Run through `sh` so the freshly written file is never exec'd (avoids ETXTBSY races).
    p.command = format!("sh {}", script.display());
    p
}

const CLI_OK: &str = r#"
printf '%s\n' '{"type":"system","subtype":"init","tools":[]}'
printf '%s\n' '{"type":"stream_event","event":{"type":"message_start","message":{}}}'
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Blue"}}}'
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"."}}}'
printf '%s\n' '{"type":"stream_event","event":{"type":"message_stop"}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"Blue."}'
"#;

#[tokio::test]
async fn claude_cli_streams_with_tools_off_and_api_keys_stripped() {
    // SAFETY: only this test in this binary reads the variable; it is set before the child starts.
    unsafe { std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-should-not-leak") };
    let dir = tempfile::tempdir().unwrap();
    let p = fake_claude(dir.path(), CLI_OK);
    let (r, out) = run(&p, "sonnet", &CancellationToken::new()).await;
    unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };
    r.unwrap();
    assert_eq!(out, "Blue.");

    let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
    let args: Vec<&str> = args.lines().collect();
    assert!(
        args.windows(2).any(|w| w == ["--tools", ""]),
        "tools disabled: {args:?}"
    );
    assert!(
        args.windows(2).any(|w| w == ["--model", "sonnet"])
            && args.windows(2).any(|w| w == ["--system-prompt", "SYS"])
    );
    assert!(
        args.contains(&"--strict-mcp-config")
            && args.contains(&"--no-session-persistence")
            && args.contains(&"-p")
    );

    let stdin = std::fs::read_to_string(dir.path().join("stdin")).unwrap();
    assert!(
        stdin.contains("The sky is blue.")
            && stdin.contains("User: earlier")
            && stdin.ends_with("User: Colour?"),
        "{stdin}"
    );
    assert!(!stdin.contains("boom"), "error rows aren't sent");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("anthropic_env"))
            .unwrap()
            .trim(),
        "0",
        "ANTHROPIC_* must not reach the CLI"
    );
    let cwd = std::fs::read_to_string(dir.path().join("cwd")).unwrap();
    assert!(
        !cwd.trim().starts_with(env!("CARGO_MANIFEST_DIR")),
        "runs in a throwaway directory, not the project: {cwd}"
    );
}

#[tokio::test]
async fn claude_cli_reports_login_and_exit_failures() {
    let dir = tempfile::tempdir().unwrap();
    let p = fake_claude(
        dir.path(),
        r#"printf '%s\n' '{"type":"result","subtype":"success","is_error":true,"result":"Invalid API key · Please run /login"}'"#,
    );
    let msg = api_error(run(&p, "default", &CancellationToken::new()).await.0);
    assert!(msg.contains("not signed in"), "{msg}");
    assert!(
        !std::fs::read_to_string(dir.path().join("args"))
            .unwrap()
            .contains("--model"),
        "`default` leaves model choice to the plan"
    );

    let dir = tempfile::tempdir().unwrap();
    let p = fake_claude(dir.path(), "echo 'something broke' >&2\nexit 3");
    let msg = api_error(run(&p, "haiku", &CancellationToken::new()).await.0);
    assert!(
        msg.contains("claude failed") && msg.contains("something broke"),
        "{msg}"
    );

    let dir = tempfile::tempdir().unwrap();
    let p = fake_claude(dir.path(), "exit 0");
    assert!(
        api_error(run(&p, "haiku", &CancellationToken::new()).await.0)
            .contains("ended unexpectedly")
    );

    let mut missing = ProviderConfig::builtin(ProviderKind::ClaudeCli);
    missing.command = "/definitely/not/here/claude".into();
    assert!(
        api_error(run(&missing, "haiku", &CancellationToken::new()).await.0)
            .contains("Could not run")
    );
    assert!(
        api_error(
            run(
                &missing,
                "--dangerously-skip-permissions",
                &CancellationToken::new()
            )
            .await
            .0
        )
        .contains("Invalid Claude model")
    );
}

#[tokio::test]
async fn claude_cli_cancel_kills_the_process() {
    let dir = tempfile::tempdir().unwrap();
    let body = r#"
echo $$ > "$D/pid"
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"partial"}}}'
exec sleep 30
"#;
    let p = fake_claude(dir.path(), body);
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        c2.cancel();
    });
    let started = Instant::now();
    let (r, out) = run(&p, "opus", &cancel).await;
    assert!(matches!(r, Err(StreamError::Cancelled)) && out == "partial");
    assert!(started.elapsed() < Duration::from_secs(5));

    let pid = std::fs::read_to_string(dir.path().join("pid"))
        .unwrap()
        .trim()
        .to_string();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Path::new(&format!("/proc/{pid}")).exists() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let state = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    assert!(
        state.is_empty() || state.contains(") Z "),
        "the CLI process must be gone (or a reaped zombie), got {state:?}"
    );
}

#[tokio::test]
async fn claude_cli_lists_aliases_without_running_anything() {
    let mut p = ProviderConfig::builtin(ProviderKind::ClaudeCli);
    p.command = "/definitely/not/here/claude".into();
    let models = provider::list_models(&p).await.unwrap();
    assert_eq!(
        models.iter().map(|m| m.reference()).collect::<Vec<_>>(),
        [
            "claude-cli::default",
            "claude-cli::sonnet",
            "claude-cli::opus",
            "claude-cli::haiku"
        ]
    );
}
