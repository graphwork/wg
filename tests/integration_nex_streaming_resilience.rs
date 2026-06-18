//! Streaming resilience tests for the OpenAI-compatible (`nex`) client.
//!
//! These reproduce — deterministically, against an in-process mock TCP SSE
//! server — the failure the user reported against a local llama.cpp endpoint:
//!
//!   [openai-client] Stream interrupted after ~6300 chunks: error decoding
//!   response body
//!
//! ## Root-cause evidence
//!
//! With reqwest 0.12, `Response::bytes_stream()` funnels *every* body error
//! through `Kind::Decode`, whose `Display` is the generic
//! "error decoding response body" — so a total-request timeout firing
//! mid-stream, a dropped connection, and a framing error are all
//! indistinguishable from the message alone. The client used to set a 300s
//! **total** request timeout, which capped a long-but-healthy generation
//! around the 300s mark (≈6300 tokens at a steady local rate) and surfaced as
//! exactly that cryptic error. The fix replaces the total timeout on the
//! streaming path with a per-read (idle) timeout, which resets on every
//! frame and so never kills a healthy stream.
//!
//! - [`total_timeout_reproduces_the_symptom`] proves a total timeout firing
//!   mid-stream yields `is_timeout() == true` *and* Display
//!   "error decoding response body" — i.e. it reproduces the symptom and
//!   shows the message hides the real cause.
//! - [`fixed_client_completes_a_long_slow_stream`] proves the shipped client
//!   (read-timeout, no total cap) rides out a slow generation that the total
//!   timeout would have cut, returning the full content.
//! - [`read_timeout_aborts_only_a_stalled_stream`] proves the read-timeout we
//!   rely on still protects against a genuinely stalled / dropped upstream,
//!   without harming a slow-but-alive one.
//! - [`connection_drop_then_retry_recovers`] proves a mid-stream connection
//!   drop is handled by the retry path and recovers seamlessly.
//! - [`split_multibyte_utf8_is_reassembled`] proves a multi-byte UTF-8
//!   sequence split across network chunks is decoded intact (the byte-buffer
//!   SSE parser), not corrupted into U+FFFD.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use futures_util::StreamExt;

use worksgood::executor::native::client::{ContentBlock, Message, MessagesRequest, Role};
use worksgood::executor::native::openai_client::OpenAiClient;
use worksgood::executor::native::provider::Provider;

// ---------------------------------------------------------------------------
// Mock SSE server (chunked transfer-encoding, controllable timing/drops)
// ---------------------------------------------------------------------------

const CHUNKED_HEADERS: &str = "HTTP/1.1 200 OK\r\n\
     Content-Type: text/event-stream\r\n\
     Cache-Control: no-cache\r\n\
     Transfer-Encoding: chunked\r\n\
     \r\n";

/// Write one HTTP chunked-transfer chunk wrapping `body` bytes.
fn write_http_chunk(stream: &mut TcpStream, body: &[u8]) -> std::io::Result<()> {
    write!(stream, "{:X}\r\n", body.len())?;
    stream.write_all(body)?;
    stream.write_all(b"\r\n")?;
    stream.flush()
}

/// Terminating zero-length chunk — marks a *clean* end of the chunked body.
fn write_http_end(stream: &mut TcpStream) -> std::io::Result<()> {
    stream.write_all(b"0\r\n\r\n")?;
    stream.flush()
}

/// Drain the inbound HTTP request (headers + small JSON body) so the client's
/// write side doesn't get reset while we stream the response back.
fn drain_request(stream: &mut TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_millis(250)))
        .ok();
    let mut buf = [0u8; 8192];
    let _ = stream.read(&mut buf);
}

/// One OpenAI streaming token frame carrying `content`.
fn token_frame(content: &str) -> String {
    format!(
        "data: {{\"id\":\"gen-mock\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":{}}},\"finish_reason\":null}}]}}\n\n",
        serde_json::to_string(content).unwrap()
    )
}

/// The final stop frame + `[DONE]` sentinel.
fn done_frames() -> String {
    "data: {\"id\":\"gen-mock\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n\
     data: [DONE]\n\n"
        .to_string()
}

/// Spawn a mock server. `handler(conn_idx, stream)` runs per accepted
/// connection (so retries can behave differently). Returns the base URL.
fn spawn_mock<F>(handler: F) -> String
where
    F: Fn(usize, &mut TcpStream) + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://127.0.0.1:{}", addr.port());

    std::thread::spawn(move || {
        let mut conn_idx = 0usize;
        for incoming in listener.incoming() {
            match incoming {
                Ok(mut stream) => {
                    handler(conn_idx, &mut stream);
                    conn_idx += 1;
                }
                Err(_) => break,
            }
        }
    });

    url
}

fn test_request() -> MessagesRequest {
    MessagesRequest {
        model: "mock-model".to_string(),
        max_tokens: 4096,
        system: Some("test".to_string()),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "stream please".to_string(),
            }],
        }],
        tools: vec![],
        stream: true,
    }
}

fn response_text(resp: &worksgood::executor::native::client::MessagesResponse) -> String {
    resp.content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 1. Total timeout firing mid-stream reproduces the exact symptom
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn total_timeout_reproduces_the_symptom() {
    // Server streams steadily for ~3s; each gap is short (200ms) so a *read*
    // timeout would never trip — only a total/wall-clock timeout can.
    let url = spawn_mock(|_idx, stream| {
        drain_request(stream);
        if stream.write_all(CHUNKED_HEADERS.as_bytes()).is_err() {
            return;
        }
        for i in 0..15 {
            if write_http_chunk(stream, token_frame(&i.to_string()).as_bytes()).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let _ = write_http_chunk(stream, done_frames().as_bytes());
        let _ = write_http_end(stream);
    });

    // A *total* request timeout — the old client's behavior — applied to the
    // streaming body. It must fire mid-stream (server runs ~3s).
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();

    let resp = client
        .post(format!("{url}/chat/completions"))
        .body("{}")
        .send()
        .await
        .expect("headers arrive before the timeout");

    let mut stream = resp.bytes_stream();
    let mut err = None;
    while let Some(item) = stream.next().await {
        if let Err(e) = item {
            err = Some(e);
            break;
        }
    }

    let err = err.expect("a total timeout must interrupt the in-flight stream");
    // The crux: the Display is the generic decode message the user saw...
    assert!(
        err.to_string().contains("error decoding response body"),
        "expected the cryptic decode message, got: {err}"
    );
    // ...yet the *real* cause is a timeout, only visible via is_timeout().
    assert!(
        err.is_timeout(),
        "the underlying cause must classify as a timeout: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 2. The shipped client rides out a long, slow-but-healthy generation
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fixed_client_completes_a_long_slow_stream() {
    // ~3s of steady streaming — longer than the old 1s-equivalent cap in
    // test #1, but every gap is short. The fixed client (read-timeout, no
    // total cap) must complete and return ALL tokens.
    let url = spawn_mock(|_idx, stream| {
        drain_request(stream);
        if stream.write_all(CHUNKED_HEADERS.as_bytes()).is_err() {
            return;
        }
        for i in 0..15 {
            if write_http_chunk(stream, token_frame(&i.to_string()).as_bytes()).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let _ = write_http_chunk(stream, done_frames().as_bytes());
        let _ = write_http_end(stream);
    });

    let client = OpenAiClient::new("test-key".to_string(), "mock-model", Some(&url))
        .unwrap()
        .with_streaming(true);

    let resp = client
        .send_streaming(&test_request(), &|_| {})
        .await
        .expect("a slow-but-alive stream must complete, not be cut by a timeout");

    // 15 tokens "0".."14" concatenated.
    assert_eq!(response_text(&resp), "01234567891011121314");
}

// ---------------------------------------------------------------------------
// 3. read_timeout aborts a stalled stream but spares a slow-but-alive one
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_timeout_aborts_only_a_stalled_stream() {
    // conn 0: healthy, short gaps -> must complete under a 500ms read-timeout.
    // conn 1: sends two frames then goes silent for 2s -> read-timeout fires.
    let url = spawn_mock(|idx, stream| {
        drain_request(stream);
        if stream.write_all(CHUNKED_HEADERS.as_bytes()).is_err() {
            return;
        }
        if idx == 0 {
            for i in 0..6 {
                if write_http_chunk(stream, token_frame(&i.to_string()).as_bytes()).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = write_http_chunk(stream, done_frames().as_bytes());
            let _ = write_http_end(stream);
        } else {
            let _ = write_http_chunk(stream, token_frame("a").as_bytes());
            let _ = write_http_chunk(stream, token_frame("b").as_bytes());
            std::thread::sleep(Duration::from_secs(2)); // stall past read-timeout
            // never sends the terminating chunk
        }
    });

    let client = reqwest::Client::builder()
        .read_timeout(Duration::from_millis(500))
        .build()
        .unwrap();

    // Healthy stream: completes without error.
    let resp = client
        .post(format!("{url}/chat/completions"))
        .body("{}")
        .send()
        .await
        .unwrap();
    let mut stream = resp.bytes_stream();
    let mut ok = true;
    while let Some(item) = stream.next().await {
        if item.is_err() {
            ok = false;
            break;
        }
    }
    assert!(ok, "a slow-but-alive stream must survive the read-timeout");

    // Stalled stream: read-timeout fires.
    let resp = client
        .post(format!("{url}/chat/completions"))
        .body("{}")
        .send()
        .await
        .unwrap();
    let mut stream = resp.bytes_stream();
    let mut err = None;
    while let Some(item) = stream.next().await {
        if let Err(e) = item {
            err = Some(e);
            break;
        }
    }
    let err = err.expect("a stalled stream must trip the read-timeout");
    assert!(
        err.is_timeout(),
        "stall must classify as a timeout: {err:?}"
    );
    assert!(err.to_string().contains("error decoding response body"));
}

// ---------------------------------------------------------------------------
// 4. A mid-stream connection drop is recovered by the retry path
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_drop_then_retry_recovers() {
    // conn 0: a few frames then an ABRUPT close (no terminating 0-chunk) ->
    //         hyper sees an incomplete chunked body -> decode error.
    // conn 1: a clean, complete stream.
    let url = spawn_mock(|idx, stream| {
        drain_request(stream);
        if stream.write_all(CHUNKED_HEADERS.as_bytes()).is_err() {
            return;
        }
        if idx == 0 {
            let _ = write_http_chunk(stream, token_frame("partial").as_bytes());
            // Drop the connection mid-stream: no `0\r\n\r\n`.
            let _ = stream.shutdown(std::net::Shutdown::Both);
        } else {
            let _ = write_http_chunk(stream, token_frame("hello ").as_bytes());
            let _ = write_http_chunk(stream, token_frame("world").as_bytes());
            let _ = write_http_chunk(stream, done_frames().as_bytes());
            let _ = write_http_end(stream);
        }
    });

    let client = OpenAiClient::new("test-key".to_string(), "mock-model", Some(&url))
        .unwrap()
        .with_streaming(true);

    let resp = client
        .send_streaming(&test_request(), &|_| {})
        .await
        .expect("the retry path must recover from a transient mid-stream drop");

    assert_eq!(response_text(&resp), "hello world");
}

// ---------------------------------------------------------------------------
// 5. Split multi-byte UTF-8 across network chunks is reassembled intact
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn split_multibyte_utf8_is_reassembled() {
    // Emit a single SSE token frame whose JSON contains multi-byte chars,
    // but split the raw bytes mid-character across two HTTP chunks with a
    // gap, forcing reqwest to surface them as separate `Bytes`.
    let url = spawn_mock(|_idx, stream| {
        drain_request(stream);
        if stream.write_all(CHUNKED_HEADERS.as_bytes()).is_err() {
            return;
        }
        let frame = token_frame("世界🌍 héllo");
        let bytes = frame.as_bytes();
        // Split one byte into the first multi-byte sequence, so the lead byte
        // is in chunk 1 and its continuation bytes are in chunk 2.
        let first_multibyte = bytes.iter().position(|&b| b >= 0x80).unwrap();
        let split = first_multibyte + 1;
        let _ = write_http_chunk(stream, &bytes[..split]);
        std::thread::sleep(Duration::from_millis(80));
        let _ = write_http_chunk(stream, &bytes[split..]);
        let _ = write_http_chunk(stream, done_frames().as_bytes());
        let _ = write_http_end(stream);
    });

    let client = OpenAiClient::new("test-key".to_string(), "mock-model", Some(&url))
        .unwrap()
        .with_streaming(true);

    let resp = client
        .send_streaming(&test_request(), &|_| {})
        .await
        .unwrap();
    let text = response_text(&resp);
    assert_eq!(text, "世界🌍 héllo");
    assert!(
        !text.contains('\u{FFFD}'),
        "multi-byte chars split across chunks must not be corrupted: {text:?}"
    );
}
