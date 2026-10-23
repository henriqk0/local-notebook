//! Exercises the client against a throwaway local HTTP server.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use rsnotebook::llm::openrouter::{ChatMessage, Client, StreamError};
use tokio_util::sync::CancellationToken;

/// Serve one connection: read the full request, then hand the socket to `respond`.
/// Returns the base URL and a channel yielding the raw request text.
fn serve(
    respond: impl FnOnce(&mut TcpStream) + Send + 'static,
) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        let mut req = Vec::new();
        let mut byte = [0u8; 1];
        while !req.ends_with(b"\r\n\r\n") {
            sock.read_exact(&mut byte).unwrap();
            req.push(byte[0]);
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

fn sse_response(sock: &mut TcpStream, chunks: &[&str]) {
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

async fn run(url: &str, cancel: &CancellationToken) -> (Result<(), StreamError>, String) {
    let client = Client::with_base_url("testkey", url);
    let mut out = String::new();
    let r = client
        .stream_chat("m/x", &[ChatMessage::new("user", "hi")], cancel, |t| {
            out.push_str(&t)
        })
        .await;
    (r, out)
}

#[tokio::test]
async fn streams_tokens_across_split_chunks_and_sends_auth() {
    let (url, rx) = serve(|s| {
        sse_response(
            s,
            &[
                "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\ndata: {\"choices\":[{\"del",
                "ta\":{\"content\":\"lo\"},\"finish_reason\":\"stop\"}]}\n\n: keep-alive\n\ndata: [DONE]\n\n",
            ],
        );
    });
    let (r, out) = run(&url, &CancellationToken::new()).await;
    r.unwrap();
    assert_eq!(out, "Hello");
    let req = rx.recv().unwrap();
    assert!(req.to_lowercase().contains("authorization: bearer testkey"));
    assert!(req.contains("\"stream\":true") && req.contains("\"model\":\"m/x\""));
}

#[tokio::test]
async fn http_errors_map_to_friendly_messages() {
    let (url, _) = serve(|s| {
        s.write_all(
            b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 4\r\nconnection: close\r\n\r\nnope",
        )
        .unwrap();
    });
    let (r, _) = run(&url, &CancellationToken::new()).await;
    match r {
        Err(StreamError::Api(e)) => {
            assert!(e.message.contains("Invalid API key") && e.status == Some(401))
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn stream_without_finish_reason_is_an_error_but_keeps_tokens() {
    let (url, _) = serve(|s| {
        sse_response(
            s,
            &["data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n"],
        )
    });
    let (r, out) = run(&url, &CancellationToken::new()).await;
    assert_eq!(out, "Hel");
    match r {
        Err(StreamError::Api(e)) => assert!(e.message.contains("ended unexpectedly")),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn mid_stream_error_object_is_reported() {
    let (url, _) = serve(|s| {
        sse_response(
            s,
            &["data: {\"error\":{\"message\":\"upstream down\"}}\n\n"],
        )
    });
    let (r, _) = run(&url, &CancellationToken::new()).await;
    match r {
        Err(StreamError::Api(e)) => assert!(e.message.contains("upstream down")),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn redirects_are_not_followed() {
    let (url, _) = serve(|s| {
        s.write_all(b"HTTP/1.1 302 Found\r\nlocation: http://127.0.0.1:1/steal\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").unwrap();
    });
    let (r, _) = run(&url, &CancellationToken::new()).await;
    match r {
        Err(StreamError::Api(e)) => assert_eq!(e.status, Some(302)),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn cancel_stops_a_stalled_stream_promptly() {
    let (url, _) = serve(|s| {
        sse_response(
            s,
            &["data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n"],
        );
        thread::sleep(Duration::from_secs(5)); // the server then goes quiet
    });
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        c2.cancel();
    });
    let started = std::time::Instant::now();
    let (r, out) = run(&url, &cancel).await;
    assert!(matches!(r, Err(StreamError::Cancelled)));
    assert_eq!(out, "partial");
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn list_models_parses_and_filters_non_objects() {
    let body = r#"{"data":[{"id":"a/b","pricing":{"prompt":"0","completion":"0"}}, 5, "x"]}"#;
    let (url, _) = serve(move |s| {
        s.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes()).unwrap();
    });
    let list = Client::with_base_url("k", &url)
        .list_models()
        .await
        .unwrap();
    assert_eq!(list.len(), 1);
}
