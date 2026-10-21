//! Real-socket tests of the server loop (ADR 0008 read timeouts task 001).
//!
//! Each test binds a real TCP listener, starts
//! `musicindex_live_relay::server::serve` with a header read timeout of
//! one second, and talks to it over `tokio::net::TcpStream` with raw
//! HTTP/1.1 bytes. No test waits more than five seconds for one result.

use std::time::Duration;

use musicindex_live_relay::{AppConfig, RelayState, app, server};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

/// The configuration of every test in this file: the default settings,
/// with a header read timeout of one second.
fn test_config() -> AppConfig {
    AppConfig {
        header_read_timeout: Duration::from_secs(1),
        ..AppConfig::for_tests()
    }
}

/// Starts the relay on `127.0.0.1:0` and returns its address.
///
/// The server loop runs on the calling test's own tokio runtime, with a
/// shutdown future that never completes. Each `#[tokio::test]` gets its
/// own runtime, so the loop ends with the test.
async fn start_relay(config: AppConfig) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a TCP listener");
    let addr = listener.local_addr().expect("read the bound address");
    let header_read_timeout = config.header_read_timeout;
    let router = app(RelayState::new(config));
    tokio::spawn(server::serve(
        listener,
        router,
        header_read_timeout,
        std::future::pending(),
    ));
    addr
}

/// Reads one HTTP/1.1 response head: the status code, the headers, and
/// any body bytes already read past the head.
///
/// Panics if the connection ends before the head ends, since every
/// response of the relay sends a complete head.
async fn read_head(stream: &mut TcpStream) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let mut buf = Vec::new();
    let head_end = loop {
        if let Some(end) = find_head_end(&buf) {
            break end;
        }
        let mut chunk = [0u8; 1024];
        let read = stream.read(&mut chunk).await.expect("read the response");
        assert!(read > 0, "the connection ended before the response head");
        buf.extend_from_slice(&chunk[..read]);
    };

    let head_text = std::str::from_utf8(&buf[..head_end]).expect("the head is UTF-8");
    let mut lines = head_text.split("\r\n");
    let status_line = lines.next().expect("a status line");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .expect("a status code")
        .parse()
        .expect("the status code is a number");
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    let body_start = buf[head_end + 4..].to_vec();
    (status, headers, body_start)
}

/// The byte index of the blank line that ends an HTTP head, or `None`
/// when `buf` does not hold one yet.
fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|window| window == b"\r\n\r\n")
}

/// Sends `request` and reads the full response by `Content-Length`.
///
/// Returns the status and the raw body bytes. Every response of the
/// relay in these tests carries `Content-Length`.
async fn request_response(stream: &mut TcpStream, request: &str) -> (u16, Vec<u8>) {
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write the request");
    let (status, headers, mut body) = read_head(stream).await;
    let content_length: usize = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .map(|(_, value)| value.parse().expect("content-length is a number"))
        .unwrap_or(0);
    while body.len() < content_length {
        let mut chunk = [0u8; 1024];
        let read = stream.read(&mut chunk).await.expect("read the body");
        assert!(read > 0, "the connection ended before the body ended");
        body.extend_from_slice(&chunk[..read]);
    }
    (status, body)
}

/// Sends `request` and reads the full response as JSON.
async fn request_json(stream: &mut TcpStream, request: &str) -> (u16, Value) {
    let (status, body) = request_response(stream, request).await;
    let value = serde_json::from_slice(&body).expect("the body is JSON");
    (status, value)
}

/// True when `needle` appears anywhere in `haystack`.
fn bytes_contain(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[tokio::test]
async fn a_request_to_health_gets_200() {
    let addr = start_relay(test_config()).await;
    let mut stream = TcpStream::connect(addr).await.expect("connect");

    let (status, body) = request_response(
        &mut stream,
        "GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(body, b"ok");
}

#[tokio::test]
async fn a_connection_with_no_full_request_head_closes_after_the_header_timeout() {
    let addr = start_relay(test_config()).await;
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: x\r\n")
        .await
        .expect("write a partial request head");

    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut probe = [0u8; 1];
    let still_open =
        tokio::time::timeout(Duration::from_millis(100), stream.read(&mut probe)).await;
    assert!(
        still_open.is_err(),
        "the connection must still be open at 0.5 seconds"
    );

    let closed = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut probe))
        .await
        .expect("the connection closes before 5 seconds");
    match closed {
        Ok(0) | Err(_) => {}
        Ok(_) => panic!("the relay must not send data for a request with no full head"),
    }
}

#[tokio::test]
async fn an_idle_keep_alive_connection_closes_after_the_header_timeout() {
    let addr = start_relay(test_config()).await;
    let mut stream = TcpStream::connect(addr).await.expect("connect");

    let (status, _) =
        request_response(&mut stream, "GET /health HTTP/1.1\r\nHost: x\r\n\r\n").await;
    assert_eq!(status, 200);

    let mut probe = [0u8; 1];
    let closed = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut probe))
        .await
        .expect("the idle connection closes before 5 seconds");
    match closed {
        Ok(0) | Err(_) => {}
        Ok(_) => panic!("the relay must not send data on an idle keep-alive connection"),
    }
}

#[tokio::test]
async fn handlers_get_the_tcp_peer_address_through_the_new_loop() {
    let config = AppConfig {
        max_listener_connections_per_client: 1,
        // Isolate the open-connections limit from the connect-rate limit.
        max_listener_connects_per_client_per_sec: 0,
        ..test_config()
    };
    let addr = start_relay(config).await;

    let mut create_stream = TcpStream::connect(addr).await.expect("connect");
    let (status, created) = request_json(
        &mut create_stream,
        "POST /v1/liveitems HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n",
    )
    .await;
    assert_eq!(status, 200);
    let event_id = created["event_id"].as_str().expect("event_id").to_string();

    // A stream from one client address succeeds, and stays open.
    let mut first = TcpStream::connect(addr).await.expect("connect");
    first
        .write_all(
            format!(
                "GET /v1/liveitems/{event_id}/events HTTP/1.1\r\nHost: x\r\n\
                 X-Real-IP: 203.0.113.1\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .expect("write the first stream request");
    let (status, _, _) = read_head(&mut first).await;
    assert_eq!(status, 200);

    // A stream from a different client address also succeeds.
    let mut second = TcpStream::connect(addr).await.expect("connect");
    second
        .write_all(
            format!(
                "GET /v1/liveitems/{event_id}/events HTTP/1.1\r\nHost: x\r\n\
                 X-Real-IP: 203.0.113.2\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .expect("write the second stream request");
    let (status, _, _) = read_head(&mut second).await;
    assert_eq!(status, 200);

    // The first client address again, while its first stream is still
    // open, is above the limit of one.
    let mut third = TcpStream::connect(addr).await.expect("connect");
    let (status, body) = request_json(
        &mut third,
        &format!(
            "GET /v1/liveitems/{event_id}/events HTTP/1.1\r\nHost: x\r\n\
             X-Real-IP: 203.0.113.1\r\n\r\n"
        ),
    )
    .await;
    assert_eq!(status, 429);
    assert_eq!(body["error"], "listener_limit_reached");

    drop(first);
    drop(second);
}

#[tokio::test]
async fn an_events_stream_stays_open_past_the_header_timeout_and_gets_a_late_publish() {
    let addr = start_relay(test_config()).await;

    let mut create_stream = TcpStream::connect(addr).await.expect("connect");
    let (status, created) = request_json(
        &mut create_stream,
        "POST /v1/liveitems HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n",
    )
    .await;
    assert_eq!(status, 200);
    let event_id = created["event_id"].as_str().expect("event_id").to_string();
    let token = created["broadcaster_token"]
        .as_str()
        .expect("broadcaster_token")
        .to_string();

    let mut sse_stream = TcpStream::connect(addr).await.expect("connect");
    sse_stream
        .write_all(
            format!("GET /v1/liveitems/{event_id}/events HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes(),
        )
        .await
        .expect("write the stream request");
    let (status, _headers, mut received) = read_head(&mut sse_stream).await;
    assert_eq!(status, 200);

    // Past the header timeout, with no request on this connection: a
    // listener stream is not an idle keep-alive connection.
    tokio::time::sleep(Duration::from_millis(2500)).await;

    let mut publish_stream = TcpStream::connect(addr).await.expect("connect");
    let body = r#"{"title":"after-the-header-timeout"}"#;
    let request = format!(
        "POST /v1/liveitems/{event_id}/metadata HTTP/1.1\r\nHost: x\r\n\
         Authorization: Bearer {token}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len(),
    );
    let (status, _) = request_json(&mut publish_stream, &request).await;
    assert_eq!(status, 200);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !bytes_contain(&received, b"after-the-header-timeout") {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            remaining > Duration::ZERO,
            "the late publish did not arrive on the stream in time"
        );
        let mut chunk = [0u8; 1024];
        let read = tokio::time::timeout(remaining, sse_stream.read(&mut chunk))
            .await
            .expect("a read before the deadline")
            .expect("read the stream");
        assert!(read > 0, "the stream closed before the publish arrived");
        received.extend_from_slice(&chunk[..read]);
    }
}

#[tokio::test]
async fn a_websocket_upgrade_gets_101_and_stays_open_past_the_header_timeout() {
    let addr = start_relay(test_config()).await;
    let mut stream = TcpStream::connect(addr).await.expect("connect");

    let request = "GET /socket.io/?EIO=4&transport=websocket HTTP/1.1\r\n\
        Host: x\r\n\
        Connection: Upgrade\r\n\
        Upgrade: websocket\r\n\
        Sec-WebSocket-Version: 13\r\n\
        Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n";
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write the upgrade request");

    let (status, _, _) = read_head(&mut stream).await;
    assert_eq!(status, 101);

    tokio::time::sleep(Duration::from_millis(2500)).await;

    let mut probe = [0u8; 256];
    match tokio::time::timeout(Duration::from_millis(200), stream.read(&mut probe)).await {
        Ok(Ok(0)) => panic!("the WebSocket connection closed"),
        Ok(Err(error)) => panic!("a read error on an open WebSocket connection: {error}"),
        Ok(Ok(_)) | Err(_) => {}
    }
}

#[tokio::test]
async fn the_http2_preface_does_not_get_an_http2_answer() {
    let addr = start_relay(test_config()).await;
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream
        .write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")
        .await
        .expect("write the HTTP/2 preface");

    let mut buf = [0u8; 256];
    let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
        .await
        .expect("a result within 5 seconds");
    match read {
        Ok(0) | Err(_) => {}
        Ok(n) => assert!(
            buf[..n].starts_with(b"HTTP/1."),
            "the relay must answer in HTTP/1.1, not {:?}",
            &buf[..n]
        ),
    }
}

/// Acceptance criterion of read timeouts task 002: a real socket, with a
/// body read timeout of one second.
#[tokio::test]
async fn a_metadata_publish_with_an_incomplete_body_gets_408_request_body_timeout() {
    let config = AppConfig {
        body_read_timeout: Duration::from_secs(1),
        ..test_config()
    };
    let addr = start_relay(config).await;

    let mut create_stream = TcpStream::connect(addr).await.expect("connect");
    let (status, created) = request_json(
        &mut create_stream,
        "POST /v1/liveitems HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n",
    )
    .await;
    assert_eq!(status, 200);
    let event_id = created["event_id"].as_str().expect("event_id").to_string();
    let token = created["broadcaster_token"]
        .as_str()
        .expect("broadcaster_token")
        .to_string();

    let mut stream = TcpStream::connect(addr).await.expect("connect");
    // `Content-Length: 100` promises 100 bytes of body. Only one byte
    // ever arrives, and the connection then sends nothing more.
    let request = format!(
        "POST /v1/liveitems/{event_id}/metadata HTTP/1.1\r\nHost: x\r\n\
         Authorization: Bearer {token}\r\nContent-Type: application/json\r\n\
         Content-Length: 100\r\n\r\nx"
    );

    let (status, body) =
        tokio::time::timeout(Duration::from_secs(5), request_json(&mut stream, &request))
            .await
            .expect("a response before 5 seconds");

    assert_eq!(status, 408);
    assert_eq!(body["error"], "request_body_timeout");
}
