use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use musicindex_live_relay::{
    AppConfig, CreateEventResponse, KeepaliveResponse, LatestMetadataResponse,
    PublishMetadataResponse, RelayState, app,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::time::timeout;
use tower::ServiceExt;

fn test_app() -> axum::Router {
    app(RelayState::new(AppConfig::for_tests()))
}

/// Builds a router backed by an injected, manually driven clock.
///
/// Returns the router, the `RelayState` handle (to call `expire_leases`
/// directly), and the clock so a test can set the current time without a
/// real sleep.
fn test_app_with(config: AppConfig) -> (axum::Router, RelayState, Arc<AtomicU64>) {
    let clock = Arc::new(AtomicU64::new(1));
    let state = RelayState::with_clock(config, {
        let clock = clock.clone();
        Arc::new(move || clock.load(Ordering::SeqCst))
    });
    (app(state.clone()), state, clock)
}

async fn read_json<T: DeserializeOwned>(response: axum::response::Response) -> T {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).expect("response body is json")
}

async fn create_event(router: axum::Router) -> CreateEventResponse {
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/liveitems")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("create response");

    assert_eq!(response.status(), StatusCode::OK);
    let created: CreateEventResponse = read_json(response).await;
    assert!(created.socket_io_url.starts_with("/event?event_id="));
    assert!(created.remote_value_url.ends_with("/remoteValue"));
    created
}

async fn create_event_at(router: axum::Router, uri: &str) -> CreateEventResponse {
    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("create response");

    assert_eq!(response.status(), StatusCode::OK);
    read_json(response).await
}

async fn publish(
    router: axum::Router,
    event_id: &str,
    token: Option<&str>,
    body: Value,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!("/v1/liveitems/{event_id}/metadata"))
        .header(header::CONTENT_TYPE, "application/json");

    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }

    router
        .oneshot(
            builder
                .body(Body::from(body.to_string()))
                .expect("publish request"),
        )
        .await
        .expect("publish response")
}

async fn keepalive(
    router: axum::Router,
    event_id: &str,
    token: Option<&str>,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!("/v1/liveitems/{event_id}/keepalive"));

    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }

    router
        .oneshot(builder.body(Body::empty()).expect("keepalive request"))
        .await
        .expect("keepalive response")
}

async fn next_sse_chunk(body: &mut Body) -> String {
    let frame = timeout(Duration::from_secs(2), body.frame())
        .await
        .expect("timed out waiting for sse frame")
        .expect("sse body frame")
        .expect("sse frame result");
    let bytes = frame.into_data().expect("sse data frame");
    String::from_utf8(bytes.to_vec()).expect("sse frame is utf-8")
}

#[tokio::test]
async fn create_publish_and_fetch_latest_snapshot() {
    let router = test_app();
    let created = create_event(router.clone()).await;

    let response = publish(
        router.clone(),
        &created.event_id,
        Some(&created.broadcaster_token),
        json!({
            "event_id": created.event_id,
            "metadata": {"title": "Now Playing", "artist": "Relay Test"}
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let accepted: PublishMetadataResponse = read_json(response).await;
    assert!(accepted.accepted);
    assert_eq!(accepted.seq, 1);

    let response = router
        .oneshot(
            Request::builder()
                .uri(format!("/v1/liveitems/{}/metadata", accepted.event_id))
                .body(Body::empty())
                .expect("latest request"),
        )
        .await
        .expect("latest response");

    assert_eq!(response.status(), StatusCode::OK);
    let latest: LatestMetadataResponse = read_json(response).await;
    assert_eq!(latest.seq, 1);
    assert_eq!(
        latest.metadata,
        json!({"title": "Now Playing", "artist": "Relay Test"})
    );
}

#[tokio::test]
async fn direct_remote_value_publish_is_accepted_and_returned_raw() {
    let router = test_app();
    let created = create_event(router.clone()).await;
    let remote_value = json!({
        "title": "This is the title for this block.",
        "image": "https://example.com/art.png",
        "line": ["this is line 1", "this is line 2"],
        "link": {
            "text": "This is the text for the link",
            "url": "https://podcastindex.social"
        },
        "description": "this would be an area for something like show notes",
        "value": {
            "model": {
                "type": "lightning",
                "method": "keysend"
            },
            "destinations": [
                {
                    "name": "The Split Kit",
                    "address": "030a58b8653d32b99200a2334cfe913e51dc7d155aa0116c176657a4f1722677a3",
                    "customKey": "696969",
                    "customValue": "boPNspwDdt7axih5DfKs",
                    "split": "5",
                    "fee": "false"
                }
            ]
        },
        "type": "person",
        "feedGuid": "optional-feed-guid",
        "itemGuid": "optional-item-guid"
    });

    let response = publish(
        router.clone(),
        &created.event_id,
        Some(&created.broadcaster_token),
        remote_value.clone(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = router
        .oneshot(
            Request::builder()
                .uri(format!("/v1/liveitems/{}/remoteValue", created.event_id))
                .body(Body::empty())
                .expect("remoteValue request"),
        )
        .await
        .expect("remoteValue response");

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = read_json(response).await;
    assert_eq!(body, remote_value);
}

#[tokio::test]
async fn direct_payload_with_event_id_and_metadata_keys_is_not_misparsed_as_wrapped() {
    let router = test_app();
    let created = create_event(router.clone()).await;

    // payload happens to have event_id + metadata keys plus extras → must be Direct
    let raw = json!({
        "event_id": "different-from-path",
        "metadata": {"foo": "bar"},
        "title": "Live show",
    });

    let response = publish(
        router.clone(),
        &created.event_id,
        Some(&created.broadcaster_token),
        raw.clone(),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);

    let response = router
        .oneshot(
            Request::builder()
                .uri(format!("/v1/liveitems/{}/remoteValue", created.event_id))
                .body(Body::empty())
                .expect("remoteValue request"),
        )
        .await
        .expect("remoteValue response");

    let body: Value = read_json(response).await;
    assert_eq!(body, raw);
}

#[tokio::test]
async fn create_accepts_trailing_slash() {
    let created = create_event_at(test_app(), "/v1/liveitems/").await;

    assert!(!created.event_id.is_empty());
    assert!(!created.broadcaster_token.is_empty());
}

#[tokio::test]
async fn publish_without_bearer_token_returns_401() {
    let router = test_app();
    let created = create_event(router.clone()).await;

    let response = publish(
        router,
        &created.event_id,
        None,
        json!({
            "event_id": created.event_id,
            "metadata": {}
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn lowercase_bearer_scheme_is_accepted() {
    let router = test_app();
    let created = create_event(router.clone()).await;

    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/liveitems/{}/metadata", created.event_id))
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::AUTHORIZATION,
                    format!("bearer {}", created.broadcaster_token),
                )
                .body(Body::from(json!({"hello": "world"}).to_string()))
                .expect("publish request"),
        )
        .await
        .expect("publish response");

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn cors_preflight_returns_allow_origin() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/v1/liveitems")
                .header("origin", "https://app.example.com")
                .header("access-control-request-method", "POST")
                .body(Body::empty())
                .expect("preflight request"),
        )
        .await
        .expect("preflight response");

    assert!(
        response
            .headers()
            .get("access-control-allow-origin")
            .is_some(),
        "missing access-control-allow-origin header"
    );
}

#[tokio::test]
async fn publish_with_wrong_token_returns_403() {
    let router = test_app();
    let created = create_event(router.clone()).await;

    let response = publish(
        router,
        &created.event_id,
        Some("wrong"),
        json!({
            "event_id": created.event_id,
            "metadata": {}
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn remote_value_returns_empty_object_before_first_publish() {
    let router = test_app();
    let created = create_event(router.clone()).await;

    let response = router
        .oneshot(
            Request::builder()
                .uri(format!("/v1/liveitems/{}/remoteValue", created.event_id))
                .body(Body::empty())
                .expect("remoteValue request"),
        )
        .await
        .expect("remoteValue response");

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = read_json(response).await;
    assert_eq!(body, json!({}));
}

#[tokio::test]
async fn publish_body_over_limit_returns_413() {
    let router = test_app();
    let created = create_event(router.clone()).await;

    // Build >64 KiB payload.
    let mut big = String::with_capacity(70 * 1024);
    big.push('"');
    for _ in 0..(70 * 1024) {
        big.push('a');
    }
    big.push('"');
    let body = format!(
        r#"{{"event_id":"{}","metadata":{{"x":{}}}}}"#,
        created.event_id, big
    );

    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/liveitems/{}/metadata", created.event_id))
                .header(header::CONTENT_TYPE, "application/json")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", created.broadcaster_token),
                )
                .body(Body::from(body))
                .expect("oversized request"),
        )
        .await
        .expect("oversized response");

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn unknown_event_id_returns_404() {
    let response = test_app()
        .oneshot(
            Request::builder()
                .uri("/v1/liveitems/missing/metadata")
                .body(Body::empty())
                .expect("latest request"),
        )
        .await
        .expect("latest response");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn health_returns_ok() {
    for uri in ["/health", "/v1/liveitems/health"] {
        let response = test_app()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .expect("health request"),
            )
            .await
            .expect("health response");

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        assert_eq!(&bytes[..], b"ok");
    }
}

#[tokio::test]
async fn sse_stream_receives_socket_io_compatible_remote_value_event() {
    let router = test_app();
    let created = create_event(router.clone()).await;

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/v1/liveitems/{}/events", created.event_id))
                .body(Body::empty())
                .expect("events request"),
        )
        .await
        .expect("events response");

    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();

    let response = publish(
        router,
        &created.event_id,
        Some(&created.broadcaster_token),
        json!({
            "event_id": created.event_id,
            "metadata": {"title": "Live"}
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let chunk = next_sse_chunk(&mut body).await;
    assert!(chunk.contains("event: remoteValue"), "{chunk}");
    assert!(chunk.contains("id: 1"), "{chunk}");
    assert!(chunk.contains("\"title\":\"Live\""), "{chunk}");
    assert!(!chunk.contains("\"metadata\""), "{chunk}");
}

#[tokio::test]
async fn reconnect_with_last_event_id_replays_only_missed_events() {
    let router = test_app();
    let created = create_event(router.clone()).await;

    for index in 1..=3 {
        let response = publish(
            router.clone(),
            &created.event_id,
            Some(&created.broadcaster_token),
            json!({
                "event_id": created.event_id,
                "metadata": {"index": index}
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    let response = router
        .oneshot(
            Request::builder()
                .uri(format!("/v1/liveitems/{}/events", created.event_id))
                .header("Last-Event-ID", "1")
                .body(Body::empty())
                .expect("events request"),
        )
        .await
        .expect("events response");

    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();

    let first = next_sse_chunk(&mut body).await;
    assert!(first.contains("id: 2"), "{first}");
    assert!(first.contains("\"index\":2"), "{first}");

    let second = next_sse_chunk(&mut body).await;
    assert!(second.contains("id: 3"), "{second}");
    assert!(second.contains("\"index\":3"), "{second}");
}

#[tokio::test]
async fn keepalive_returns_200_with_lease_fields() {
    let (router, _state, _clock) = test_app_with(AppConfig::for_tests());
    let created = create_event(router.clone()).await;

    let response = publish(
        router.clone(),
        &created.event_id,
        Some(&created.broadcaster_token),
        json!({"title": "Live"}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = keepalive(router, &created.event_id, Some(&created.broadcaster_token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: KeepaliveResponse = read_json(response).await;
    assert_eq!(body.event_id, created.event_id);
    assert!(!body.lease_expires_at.is_empty());
    assert_eq!(body.keepalive_interval_secs, 30);
}

#[tokio::test]
async fn keepalive_without_bearer_token_returns_401() {
    let (router, _state, _clock) = test_app_with(AppConfig::for_tests());
    let created = create_event(router.clone()).await;
    publish(
        router.clone(),
        &created.event_id,
        Some(&created.broadcaster_token),
        json!({"title": "Live"}),
    )
    .await;

    let response = keepalive(router, &created.event_id, None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn keepalive_with_wrong_token_returns_403() {
    let (router, _state, _clock) = test_app_with(AppConfig::for_tests());
    let created = create_event(router.clone()).await;
    publish(
        router.clone(),
        &created.event_id,
        Some(&created.broadcaster_token),
        json!({"title": "Live"}),
    )
    .await;

    let response = keepalive(router, &created.event_id, Some("wrong")).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn keepalive_for_unknown_event_returns_404() {
    let (router, _state, _clock) = test_app_with(AppConfig::for_tests());

    let response = keepalive(router, "missing", Some("token")).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn keepalive_with_no_snapshot_returns_409() {
    let (router, _state, _clock) = test_app_with(AppConfig::for_tests());
    let created = create_event(router.clone()).await;

    let response = keepalive(
        router.clone(),
        &created.event_id,
        Some(&created.broadcaster_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = read_json(response).await;
    assert_eq!(body["error"], "lease_expired");

    let response = router
        .oneshot(
            Request::builder()
                .uri(format!("/v1/liveitems/{}/remoteValue", created.event_id))
                .body(Body::empty())
                .expect("remoteValue request"),
        )
        .await
        .expect("remoteValue response");
    let body: Value = read_json(response).await;
    assert_eq!(body, json!({}));
}

#[tokio::test]
async fn keepalive_after_expiry_returns_409_and_does_not_restore_snapshot() {
    let (router, state, clock) = test_app_with(AppConfig {
        lease: Duration::from_secs(10),
        ..AppConfig::for_tests()
    });
    let created = create_event(router.clone()).await;

    let response = publish(
        router.clone(),
        &created.event_id,
        Some(&created.broadcaster_token),
        json!({"title": "Live"}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    clock.store(11, Ordering::SeqCst); // renewed_at=1, now-renewed_at=10 >= lease
    assert_eq!(state.expire_leases().await, 1);

    let response = keepalive(
        router.clone(),
        &created.event_id,
        Some(&created.broadcaster_token),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = read_json(response).await;
    assert_eq!(body["error"], "lease_expired");

    let response = router
        .oneshot(
            Request::builder()
                .uri(format!("/v1/liveitems/{}/remoteValue", created.event_id))
                .body(Body::empty())
                .expect("remoteValue request"),
        )
        .await
        .expect("remoteValue response");
    let body: Value = read_json(response).await;
    assert_eq!(body, json!({}), "the removed snapshot must not come back");
}

#[tokio::test]
async fn keepalive_returns_429_when_rate_limited() {
    let (router, _state, _clock) = test_app_with(AppConfig {
        max_publishes_per_event_per_sec: 1,
        ..AppConfig::for_tests()
    });
    let created = create_event(router.clone()).await;

    let response = publish(
        router.clone(),
        &created.event_id,
        Some(&created.broadcaster_token),
        json!({"title": "Live"}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = keepalive(router, &created.event_id, Some(&created.broadcaster_token)).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn latest_metadata_includes_renewed_at_and_lease_expires_at() {
    let (router, _state, _clock) = test_app_with(AppConfig::for_tests());
    let created = create_event(router.clone()).await;

    let response = publish(
        router.clone(),
        &created.event_id,
        Some(&created.broadcaster_token),
        json!({"title": "Live"}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = router
        .oneshot(
            Request::builder()
                .uri(format!("/v1/liveitems/{}/metadata", created.event_id))
                .body(Body::empty())
                .expect("latest request"),
        )
        .await
        .expect("latest response");

    assert_eq!(response.status(), StatusCode::OK);
    let body: LatestMetadataResponse = read_json(response).await;
    assert!(!body.renewed_at.is_empty());
    assert!(!body.lease_expires_at.is_empty());
}

/// Tests for the reserved live items of ADR 0001: the reserve route
/// (task 002), and the restore and the TTL exemption (task 003).
mod reserved {
    use std::path::{Path, PathBuf};

    use musicindex_live_relay::{AdminToken, ReserveEventResponse};
    use tempfile::TempDir;

    use super::*;

    const ADMIN_TOKEN: &str = "test-admin-token-0123456789";

    /// Builds a router with an admin token and a state file in a temporary
    /// directory. Keep the `TempDir` until the test ends.
    fn reserved_app_with(max_reserved_items: usize) -> (axum::Router, TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_file = dir.path().join("reserved-items.sqlite3");
        let state = RelayState::try_new(AppConfig {
            admin_token: Some(AdminToken::new(ADMIN_TOKEN)),
            state_file: state_file.clone(),
            max_reserved_items,
            ..AppConfig::for_tests()
        })
        .expect("open state");
        (app(state), dir, state_file)
    }

    fn reserved_app() -> (axum::Router, TempDir, PathBuf) {
        reserved_app_with(100)
    }

    async fn reserve(
        router: axum::Router,
        credential: Option<&str>,
        body: Option<Value>,
    ) -> axum::response::Response {
        let mut builder = Request::builder()
            .method("POST")
            .uri("/v1/liveitems/reserved");
        if let Some(credential) = credential {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {credential}"));
        }
        let body = match body {
            Some(body) => {
                builder = builder.header(header::CONTENT_TYPE, "application/json");
                Body::from(body.to_string())
            }
            None => Body::empty(),
        };
        router
            .oneshot(builder.body(body).expect("reserve request"))
            .await
            .expect("reserve response")
    }

    async fn reserve_ok(router: axum::Router, label: Option<&str>) -> ReserveEventResponse {
        let body = label.map(|label| json!({ "label": label }));
        let response = reserve(router, Some(ADMIN_TOKEN), body).await;
        assert_eq!(response.status(), StatusCode::CREATED);
        read_json(response).await
    }

    async fn error_code(response: axum::response::Response) -> String {
        let body: Value = read_json(response).await;
        body["error"].as_str().expect("error code").to_string()
    }

    fn table_columns(path: &Path, table: &str) -> Vec<String> {
        let connection = rusqlite::Connection::open(path).expect("open state file");
        let mut statement = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .expect("prepare");
        statement
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("columns")
    }

    fn reserved_row_count(path: &Path) -> i64 {
        rusqlite::Connection::open(path)
            .expect("open state file")
            .query_row("SELECT COUNT(*) FROM live_items", [], |row| row.get(0))
            .expect("count")
    }

    #[tokio::test]
    async fn reserve_with_correct_credential_returns_201_and_the_contract_fields() {
        let (router, _dir, state_file) = reserved_app();

        let response = reserve(
            router,
            Some(ADMIN_TOKEN),
            Some(json!({ "label": "weekly show" })),
        )
        .await;

        assert_eq!(response.status(), StatusCode::CREATED);
        let body: Value = read_json(response).await;
        let object = body.as_object().expect("object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "broadcaster_token",
                "event_id",
                "events_url",
                "label",
                "metadata_url",
                "remote_value_url",
                "socket_io_url",
            ]
        );
        let event_id = body["event_id"].as_str().expect("event_id");
        assert!(!event_id.is_empty());
        assert!(
            !body["broadcaster_token"]
                .as_str()
                .expect("token")
                .is_empty()
        );
        assert_eq!(
            body["metadata_url"],
            format!("/v1/liveitems/{event_id}/metadata")
        );
        assert_eq!(
            body["events_url"],
            format!("/v1/liveitems/{event_id}/events")
        );
        assert_eq!(body["label"], "weekly show");
        assert_eq!(reserved_row_count(&state_file), 1);
    }

    #[tokio::test]
    async fn reserve_with_no_body_omits_the_label() {
        let (router, _dir, _state_file) = reserved_app();

        let response = reserve(router, Some(ADMIN_TOKEN), None).await;

        assert_eq!(response.status(), StatusCode::CREATED);
        let body: Value = read_json(response).await;
        assert!(body.get("label").is_none());
        assert!(body.get("event_id").is_some());
    }

    #[tokio::test]
    async fn reserve_with_wrong_credential_returns_403() {
        let (router, _dir, state_file) = reserved_app();

        let response = reserve(
            router,
            Some("wrong-admin-token-0123456789"),
            Some(json!({ "label": "show" })),
        )
        .await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(error_code(response).await, "invalid_admin_token");
        assert_eq!(reserved_row_count(&state_file), 0);
    }

    #[tokio::test]
    async fn reserve_with_missing_credential_returns_401() {
        let (router, _dir, state_file) = reserved_app();

        let response = reserve(router, None, Some(json!({ "label": "show" }))).await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(reserved_row_count(&state_file), 0);
    }

    #[tokio::test]
    async fn reserve_with_a_broadcaster_token_returns_403() {
        let (router, _dir, _state_file) = reserved_app();
        let created = create_event(router.clone()).await;

        let response = reserve(router, Some(&created.broadcaster_token), None).await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn reserve_with_no_admin_token_configured_returns_404() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_file = dir.path().join("reserved-items.sqlite3");
        let router = app(RelayState::new(AppConfig {
            state_file: state_file.clone(),
            ..AppConfig::for_tests()
        }));

        let response = reserve(router, Some(ADMIN_TOKEN), Some(json!({ "label": "show" }))).await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(error_code(response).await, "reserved_items_disabled");
        assert!(!state_file.exists(), "a disabled feature opens no file");
    }

    #[tokio::test]
    async fn reserve_with_a_duplicate_label_returns_409() {
        let (router, _dir, state_file) = reserved_app();
        reserve_ok(router.clone(), Some("weekly show")).await;

        let response = reserve(
            router,
            Some(ADMIN_TOKEN),
            Some(json!({ "label": "weekly show" })),
        )
        .await;

        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(error_code(response).await, "label_already_reserved");
        assert_eq!(reserved_row_count(&state_file), 1);
    }

    #[tokio::test]
    async fn reserve_with_an_invalid_label_returns_400() {
        let (router, _dir, state_file) = reserved_app();

        for body in [
            json!({ "label": "" }),
            json!({ "label": "x".repeat(201) }),
            json!({ "label": 7 }),
            json!({ "name": "show" }),
        ] {
            let response = reserve(router.clone(), Some(ADMIN_TOKEN), Some(body)).await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
        assert_eq!(reserved_row_count(&state_file), 0);
    }

    #[tokio::test]
    async fn reserve_over_the_limit_returns_503() {
        let (router, _dir, _state_file) = reserved_app_with(1);
        reserve_ok(router.clone(), None).await;

        let response = reserve(router, Some(ADMIN_TOKEN), None).await;

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error_code(response).await, "max_reserved_items_reached");
    }

    #[tokio::test]
    async fn reserve_store_failure_returns_500_with_no_detail() {
        let (router, _dir, state_file) = reserved_app();
        rusqlite::Connection::open(&state_file)
            .expect("open state file")
            .execute("DROP TABLE live_items", [])
            .expect("drop table");

        let response = reserve(router.clone(), Some(ADMIN_TOKEN), None).await;

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body: Value = read_json(response).await;
        assert_eq!(body, json!({ "error": "store_unavailable" }));

        // A failed reserve leaves the ephemeral route working.
        create_event(router).await;
    }

    #[tokio::test]
    async fn publish_to_a_reserved_item_with_its_broadcaster_token() {
        let (router, _dir, _state_file) = reserved_app();
        let reserved = reserve_ok(router.clone(), Some("station")).await;

        let response = publish(
            router.clone(),
            &reserved.event_id,
            Some(&reserved.broadcaster_token),
            json!({ "title": "on air", "value": { "destinations": [] } }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let published: PublishMetadataResponse = read_json(response).await;
        assert_eq!(published.seq, 1);

        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/liveitems/{}/remoteValue", reserved.event_id))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value = read_json(response).await;
        assert_eq!(value["title"], "on air");

        let response = publish(
            router,
            &reserved.event_id,
            Some("wrong"),
            json!({ "title": "x" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn ephemeral_create_writes_nothing_to_the_state_file() {
        let (router, _dir, state_file) = reserved_app();

        let created = create_event(router).await;

        assert!(!created.event_id.is_empty());
        assert_eq!(reserved_row_count(&state_file), 0);
    }

    #[tokio::test]
    async fn state_file_holds_identity_and_hash_only() {
        let (router, dir, state_file) = reserved_app();
        let reserved = reserve_ok(router.clone(), Some("weekly show")).await;
        let marker = "payload-marker-7f3a9c";
        let response = publish(
            router,
            &reserved.event_id,
            Some(&reserved.broadcaster_token),
            json!({ "title": marker, "value": { "destinations": [] } }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        assert_eq!(table_columns(&state_file, "schema_version"), ["version"]);
        assert_eq!(
            table_columns(&state_file, "live_items"),
            ["event_id", "token_hash", "label", "created_at", "class"]
        );

        // No file in the state directory holds the payload, the token, or
        // the admin token.
        for entry in std::fs::read_dir(dir.path()).expect("read state directory") {
            let bytes = std::fs::read(entry.expect("entry").path()).expect("read file");
            let text = String::from_utf8_lossy(&bytes);
            assert!(!text.contains(marker), "the payload reached the disk");
            assert!(
                !text.contains(&reserved.broadcaster_token),
                "the broadcaster token reached the disk"
            );
            assert!(
                !text.contains(ADMIN_TOKEN),
                "the admin token reached the disk"
            );
        }
    }

    #[tokio::test]
    async fn admin_token_debug_output_is_redacted() {
        let config = AppConfig {
            admin_token: Some(AdminToken::new(ADMIN_TOKEN)),
            ..AppConfig::for_tests()
        };

        let output = format!("{config:?}");

        assert!(!output.contains(ADMIN_TOKEN));
        assert!(output.contains("<redacted>"));
    }

    /// Builds a relay state with an admin token, the state file `state_file`,
    /// a TTL of 10 seconds, a lease of 10 seconds, and an injected clock.
    fn reserved_state_at(
        state_file: &Path,
        clock: &Arc<AtomicU64>,
    ) -> Result<RelayState, musicindex_live_relay::store::StoreError> {
        RelayState::try_with_clock(
            AppConfig {
                admin_token: Some(AdminToken::new(ADMIN_TOKEN)),
                state_file: state_file.to_path_buf(),
                ttl: Duration::from_secs(10),
                lease: Duration::from_secs(10),
                ..AppConfig::for_tests()
            },
            {
                let clock = clock.clone();
                Arc::new(move || clock.load(Ordering::SeqCst))
            },
        )
    }

    async fn get(router: axum::Router, uri: String) -> axum::response::Response {
        router
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response")
    }

    /// Reserves an item, publishes `marker` to it, and stops the relay.
    /// Returns the reserve response.
    async fn reserve_publish_and_stop(state_file: &Path, marker: &str) -> ReserveEventResponse {
        let clock = Arc::new(AtomicU64::new(1_000));
        let state = reserved_state_at(state_file, &clock).expect("open state");
        let router = app(state);
        let reserved = reserve_ok(router.clone(), Some("station")).await;
        for _ in 0..3 {
            let response = publish(
                router.clone(),
                &reserved.event_id,
                Some(&reserved.broadcaster_token),
                json!({ "title": marker, "value": { "destinations": [] } }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
        }
        reserved
    }

    #[tokio::test]
    async fn reserved_token_validates_after_a_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_file = dir.path().join("reserved-items.sqlite3");
        let reserved = reserve_publish_and_stop(&state_file, "before restart").await;

        let clock = Arc::new(AtomicU64::new(5_000));
        let router = app(reserved_state_at(&state_file, &clock).expect("restore"));

        let response = publish(
            router.clone(),
            &reserved.event_id,
            Some("wrong"),
            json!({ "title": "x" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let response = publish(
            router,
            &reserved.event_id,
            Some(&reserved.broadcaster_token),
            json!({ "title": "after restart" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let published: PublishMetadataResponse = read_json(response).await;
        // The sequence number restarts at zero, so the first publish gives 1.
        assert_eq!(published.seq, 1);
    }

    #[tokio::test]
    async fn restored_item_serves_no_previous_payload() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_file = dir.path().join("reserved-items.sqlite3");
        let marker = "payload-before-restart-41c2";
        let reserved = reserve_publish_and_stop(&state_file, marker).await;

        let clock = Arc::new(AtomicU64::new(5_000));
        let router = app(reserved_state_at(&state_file, &clock).expect("restore"));

        // The metadata read gives the answer of an item with no snapshot.
        let response = get(
            router.clone(),
            format!("/v1/liveitems/{}/metadata", reserved.event_id),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body: Value = read_json(response).await;
        assert_eq!(body, json!({ "error": "metadata_not_found" }));

        let response = get(
            router.clone(),
            format!("/v1/liveitems/{}/remoteValue", reserved.event_id),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value = read_json(response).await;
        assert_eq!(value, json!({}));

        // An SSE client that keeps an old `Last-Event-ID` gets no replay.
        let response = router
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/liveitems/{}/events", reserved.event_id))
                    .header("last-event-id", "0")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body();
        let chunk = timeout(Duration::from_millis(200), body.frame()).await;
        if let Ok(Some(Ok(frame))) = chunk
            && let Some(data) = frame.data_ref()
        {
            let text = String::from_utf8_lossy(data);
            assert!(
                !text.contains(marker),
                "the SSE replay held the old payload"
            );
            assert!(!text.contains("remoteValue"), "the SSE replay is not empty");
        }
    }

    #[tokio::test]
    async fn keepalive_on_a_restored_item_returns_409_until_a_publish() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_file = dir.path().join("reserved-items.sqlite3");
        let reserved = reserve_publish_and_stop(&state_file, "before restart").await;

        let clock = Arc::new(AtomicU64::new(5_000));
        let router = app(reserved_state_at(&state_file, &clock).expect("restore"));

        let response = keepalive(
            router.clone(),
            &reserved.event_id,
            Some(&reserved.broadcaster_token),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(error_code(response).await, "lease_expired");

        let response = publish(
            router.clone(),
            &reserved.event_id,
            Some(&reserved.broadcaster_token),
            json!({ "title": "back on air" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let response = keepalive(
            router,
            &reserved.event_id,
            Some(&reserved.broadcaster_token),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn reaper_removes_an_ephemeral_item_and_keeps_a_reserved_item() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_file = dir.path().join("reserved-items.sqlite3");
        let clock = Arc::new(AtomicU64::new(100));
        let state = reserved_state_at(&state_file, &clock).expect("open state");
        let router = app(state.clone());
        let reserved = reserve_ok(router.clone(), Some("station")).await;
        let ephemeral = create_event(router.clone()).await;

        // The TTL is 10 seconds. Both items are idle for 11 seconds.
        clock.store(111, Ordering::SeqCst);
        assert_eq!(state.cleanup_expired().await, 1);

        let response = get(
            router.clone(),
            format!("/v1/liveitems/{}/remoteValue", ephemeral.event_id),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = get(
            router.clone(),
            format!("/v1/liveitems/{}/remoteValue", reserved.event_id),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(reserved_row_count(&state_file), 1);

        let response = publish(
            router,
            &reserved.event_id,
            Some(&reserved.broadcaster_token),
            json!({ "title": "still here" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn reaper_keeps_a_restored_item() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_file = dir.path().join("reserved-items.sqlite3");
        let reserved = reserve_publish_and_stop(&state_file, "before restart").await;

        let clock = Arc::new(AtomicU64::new(5_000));
        let state = reserved_state_at(&state_file, &clock).expect("restore");
        clock.store(5_000 + 1_000_000, Ordering::SeqCst);

        assert_eq!(state.cleanup_expired().await, 0);
        let response = get(
            app(state),
            format!("/v1/liveitems/{}/remoteValue", reserved.event_id),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(reserved_row_count(&state_file), 1);
    }

    #[tokio::test]
    async fn lease_expiry_on_a_reserved_item_removes_the_snapshot_and_keeps_the_item() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_file = dir.path().join("reserved-items.sqlite3");
        let clock = Arc::new(AtomicU64::new(100));
        let state = reserved_state_at(&state_file, &clock).expect("open state");
        let router = app(state.clone());
        let reserved = reserve_ok(router.clone(), None).await;
        let response = publish(
            router.clone(),
            &reserved.event_id,
            Some(&reserved.broadcaster_token),
            json!({ "title": "on air" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // The lease is 10 seconds. The TTL is also 10 seconds.
        clock.store(111, Ordering::SeqCst);
        assert_eq!(state.expire_leases().await, 1);
        assert_eq!(state.cleanup_expired().await, 0);

        let response = get(
            router.clone(),
            format!("/v1/liveitems/{}/metadata", reserved.event_id),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(error_code(response).await, "metadata_not_found");

        let response = get(
            router,
            format!("/v1/liveitems/{}/remoteValue", reserved.event_id),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value = read_json(response).await;
        assert_eq!(value, json!({}));
        assert_eq!(reserved_row_count(&state_file), 1);
    }

    #[tokio::test]
    async fn corrupt_state_file_fails_startup_and_stays_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_file = dir.path().join("reserved-items.sqlite3");
        let garbage = b"not a SQLite database \x00\x01\x02".repeat(128);
        std::fs::write(&state_file, &garbage).expect("write garbage");

        let clock = Arc::new(AtomicU64::new(1));
        let Err(err) = reserved_state_at(&state_file, &clock) else {
            panic!("a corrupt state file must fail startup");
        };

        assert!(
            err.to_string().contains(&state_file.display().to_string()),
            "the error names the path: {err}"
        );
        assert_eq!(std::fs::read(&state_file).expect("read"), garbage);
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(
            names,
            vec![state_file.file_name().expect("name").to_owned()]
        );
    }

    #[tokio::test]
    async fn state_file_with_no_item_table_fails_startup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_file = dir.path().join("reserved-items.sqlite3");
        drop(reserved_state_at(&state_file, &Arc::new(AtomicU64::new(1))).expect("open"));
        rusqlite::Connection::open(&state_file)
            .expect("open state file")
            .execute("DROP TABLE live_items", [])
            .expect("drop table");

        let Err(err) = reserved_state_at(&state_file, &Arc::new(AtomicU64::new(1))) else {
            panic!("a state file with no item table must fail startup");
        };
        assert!(err.to_string().contains(&state_file.display().to_string()));
    }

    #[tokio::test]
    async fn no_admin_token_restores_nothing_and_leaves_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state_file = dir.path().join("reserved-items.sqlite3");
        let reserved = reserve_publish_and_stop(&state_file, "before restart").await;
        let before = std::fs::read(&state_file).expect("read");

        let router = app(RelayState::try_new(AppConfig {
            state_file: state_file.clone(),
            ..AppConfig::for_tests()
        })
        .expect("start with no admin token"));

        let response = get(
            router,
            format!("/v1/liveitems/{}/remoteValue", reserved.event_id),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(std::fs::read(&state_file).expect("read"), before);
    }

    /// Tests for the list and delete routes of ADR 0001 task 004.
    mod list_and_delete {
        use super::*;

        async fn list(router: axum::Router, credential: Option<&str>) -> axum::response::Response {
            let mut builder = Request::builder().uri("/v1/liveitems/reserved");
            if let Some(credential) = credential {
                builder = builder.header(header::AUTHORIZATION, format!("Bearer {credential}"));
            }
            router
                .oneshot(builder.body(Body::empty()).expect("list request"))
                .await
                .expect("list response")
        }

        async fn list_ok(router: axum::Router) -> Value {
            let response = list(router, Some(ADMIN_TOKEN)).await;
            assert_eq!(response.status(), StatusCode::OK);
            read_json(response).await
        }

        async fn delete(
            router: axum::Router,
            event_id: &str,
            credential: Option<&str>,
        ) -> axum::response::Response {
            let mut builder = Request::builder()
                .method("DELETE")
                .uri(format!("/v1/liveitems/reserved/{event_id}"));
            if let Some(credential) = credential {
                builder = builder.header(header::AUTHORIZATION, format!("Bearer {credential}"));
            }
            router
                .oneshot(builder.body(Body::empty()).expect("delete request"))
                .await
                .expect("delete response")
        }

        fn listed_ids(body: &Value) -> Vec<String> {
            body["reserved"]
                .as_array()
                .expect("reserved array")
                .iter()
                .map(|item| item["event_id"].as_str().expect("event_id").to_string())
                .collect()
        }

        fn stored_hash(path: &Path, event_id: &str) -> Vec<u8> {
            rusqlite::Connection::open(path)
                .expect("open state file")
                .query_row(
                    "SELECT token_hash FROM live_items WHERE event_id = ?1",
                    [event_id],
                    |row| row.get(0),
                )
                .expect("token hash")
        }

        fn timestamp(seconds: i64) -> String {
            time::OffsetDateTime::from_unix_timestamp(seconds)
                .expect("timestamp")
                .format(&time::format_description::well_known::Rfc3339)
                .expect("format")
        }

        #[tokio::test]
        async fn list_with_correct_credential_returns_only_reserved_items() {
            let (router, _dir, _state_file) = reserved_app();
            let first = reserve_ok(router.clone(), Some("weekly show")).await;
            let second = reserve_ok(router.clone(), None).await;
            let ephemeral = create_event(router.clone()).await;

            let body = list_ok(router).await;

            let mut ids = listed_ids(&body);
            ids.sort();
            let mut expected = vec![first.event_id.clone(), second.event_id.clone()];
            expected.sort();
            assert_eq!(ids, expected);
            assert!(!ids.contains(&ephemeral.event_id));
            let items = body["reserved"].as_array().expect("array");
            let labelled = items
                .iter()
                .find(|item| item["event_id"] == first.event_id.as_str())
                .expect("first item");
            assert_eq!(labelled["label"], "weekly show");
            let unlabelled = items
                .iter()
                .find(|item| item["event_id"] == second.event_id.as_str())
                .expect("second item");
            assert!(unlabelled.get("label").is_none());
        }

        #[tokio::test]
        async fn list_with_no_reserved_item_keeps_the_key() {
            let (router, _dir, _state_file) = reserved_app();
            create_event(router.clone()).await;

            let body = list_ok(router).await;

            assert_eq!(body, json!({ "reserved": [] }));
        }

        #[tokio::test]
        async fn list_holds_no_token_and_no_hash() {
            let (router, _dir, state_file) = reserved_app();
            let reserved = reserve_ok(router.clone(), Some("station")).await;
            let response = publish(
                router.clone(),
                &reserved.event_id,
                Some(&reserved.broadcaster_token),
                json!({ "title": "on air" }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);

            let response = list(router, Some(ADMIN_TOKEN)).await;
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("read body");
            let text = String::from_utf8(bytes.to_vec()).expect("utf-8");
            let body: Value = serde_json::from_str(&text).expect("json");

            assert_eq!(body.as_object().expect("object").len(), 1);
            let item = body["reserved"][0].as_object().expect("item");
            let mut keys: Vec<&str> = item.keys().map(String::as_str).collect();
            keys.sort_unstable();
            assert_eq!(
                keys,
                vec!["created_at", "event_id", "label", "last_publish_at"]
            );

            let hash = stored_hash(&state_file, &reserved.event_id);
            let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
            let byte_list = serde_json::to_string(&hash).expect("bytes");
            assert!(!text.contains(&reserved.broadcaster_token), "token in list");
            assert!(!text.contains(ADMIN_TOKEN), "admin token in list");
            assert!(!text.to_lowercase().contains(&hex), "hex hash in list");
            assert!(
                !text.contains(&byte_list[1..byte_list.len() - 1]),
                "hash bytes in list"
            );
            assert!(!text.contains("token"), "a token field in list");
            assert!(!text.contains("hash"), "a hash field in list");
        }

        #[tokio::test]
        async fn list_reports_the_last_publish_and_not_a_keepalive() {
            let dir = tempfile::tempdir().expect("tempdir");
            let state_file = dir.path().join("reserved-items.sqlite3");
            let clock = Arc::new(AtomicU64::new(1_000));
            let router = app(reserved_state_at(&state_file, &clock).expect("open state"));
            let reserved = reserve_ok(router.clone(), None).await;

            let body = list_ok(router.clone()).await;
            let item = &body["reserved"][0];
            assert_eq!(item["created_at"], timestamp(1_000));
            assert!(
                item.get("last_publish_at").is_none(),
                "absent before the first publish: {item}"
            );

            clock.store(1_005, Ordering::SeqCst);
            let response = publish(
                router.clone(),
                &reserved.event_id,
                Some(&reserved.broadcaster_token),
                json!({ "title": "on air" }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);

            clock.store(1_008, Ordering::SeqCst);
            let response = keepalive(
                router.clone(),
                &reserved.event_id,
                Some(&reserved.broadcaster_token),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);

            // A rejected publish does not move the time.
            let response = publish(
                router.clone(),
                &reserved.event_id,
                Some("wrong"),
                json!({ "title": "x" }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);

            let body = list_ok(router).await;
            let item = &body["reserved"][0];
            assert_eq!(item["created_at"], timestamp(1_000));
            assert_eq!(item["last_publish_at"], timestamp(1_005));
        }

        #[tokio::test]
        async fn list_after_a_restart_has_no_last_publish() {
            let dir = tempfile::tempdir().expect("tempdir");
            let state_file = dir.path().join("reserved-items.sqlite3");
            let reserved = reserve_publish_and_stop(&state_file, "before restart").await;

            let clock = Arc::new(AtomicU64::new(5_000));
            let router = app(reserved_state_at(&state_file, &clock).expect("restore"));
            let body = list_ok(router).await;

            assert_eq!(listed_ids(&body), vec![reserved.event_id]);
            let item = &body["reserved"][0];
            assert_eq!(item["label"], "station");
            assert_eq!(item["created_at"], timestamp(1_000));
            assert!(item.get("last_publish_at").is_none(), "{item}");
        }

        #[tokio::test]
        async fn delete_removes_the_item_and_a_later_read_answers_404() {
            let dir = tempfile::tempdir().expect("tempdir");
            let state_file = dir.path().join("reserved-items.sqlite3");
            let clock = Arc::new(AtomicU64::new(1_000));
            let router = app(reserved_state_at(&state_file, &clock).expect("open state"));
            let reserved = reserve_ok(router.clone(), Some("station")).await;
            let kept = reserve_ok(router.clone(), Some("other")).await;
            let response = publish(
                router.clone(),
                &reserved.event_id,
                Some(&reserved.broadcaster_token),
                json!({ "title": "on air" }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);

            let response = delete(router.clone(), &reserved.event_id, Some(ADMIN_TOKEN)).await;
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
            let bytes = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("read body");
            assert!(bytes.is_empty());

            for route in ["metadata", "remoteValue", "events"] {
                let response = get(
                    router.clone(),
                    format!("/v1/liveitems/{}/{route}", reserved.event_id),
                )
                .await;
                assert_eq!(response.status(), StatusCode::NOT_FOUND, "{route}");
                assert_eq!(error_code(response).await, "event_not_found", "{route}");
            }
            let response = publish(
                router.clone(),
                &reserved.event_id,
                Some(&reserved.broadcaster_token),
                json!({ "title": "x" }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            let response = keepalive(
                router.clone(),
                &reserved.event_id,
                Some(&reserved.broadcaster_token),
            )
            .await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            let response = delete(router.clone(), &reserved.event_id, Some(ADMIN_TOKEN)).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);

            assert_eq!(
                listed_ids(&list_ok(router).await),
                vec![kept.event_id.clone()]
            );
            assert_eq!(reserved_row_count(&state_file), 1);

            // The delete is permanent. A restart does not bring the item back.
            let router = app(reserved_state_at(&state_file, &clock).expect("restore"));
            let response = get(
                router.clone(),
                format!("/v1/liveitems/{}/remoteValue", reserved.event_id),
            )
            .await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert_eq!(listed_ids(&list_ok(router).await), vec![kept.event_id]);
        }

        #[tokio::test]
        async fn delete_of_an_ephemeral_identifier_answers_404_and_removes_nothing() {
            let (router, _dir, state_file) = reserved_app();
            reserve_ok(router.clone(), None).await;
            let ephemeral = create_event(router.clone()).await;
            let response = publish(
                router.clone(),
                &ephemeral.event_id,
                Some(&ephemeral.broadcaster_token),
                json!({ "title": "ephemeral show" }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);

            let response = delete(router.clone(), &ephemeral.event_id, Some(ADMIN_TOKEN)).await;
            let ephemeral_status = response.status();
            let ephemeral_body: Value = read_json(response).await;
            let response = delete(router.clone(), "no-such-item", Some(ADMIN_TOKEN)).await;
            let absent_status = response.status();
            let absent_body: Value = read_json(response).await;

            // An ephemeral identifier gives the same answer as an absent one.
            assert_eq!(ephemeral_status, StatusCode::NOT_FOUND);
            assert_eq!(ephemeral_status, absent_status);
            assert_eq!(ephemeral_body, absent_body);

            let response = get(
                router.clone(),
                format!("/v1/liveitems/{}/remoteValue", ephemeral.event_id),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let value: Value = read_json(response).await;
            assert_eq!(value, json!({ "title": "ephemeral show" }));
            let response = publish(
                router,
                &ephemeral.event_id,
                Some(&ephemeral.broadcaster_token),
                json!({ "title": "still here" }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(reserved_row_count(&state_file), 1);
        }

        #[tokio::test]
        async fn both_routes_answer_401_and_403_for_a_bad_credential() {
            let (router, _dir, state_file) = reserved_app();
            let reserved = reserve_ok(router.clone(), None).await;
            let ephemeral = create_event(router.clone()).await;

            let response = list(router.clone(), None).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            let response = delete(router.clone(), &reserved.event_id, None).await;
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

            for credential in [
                "wrong-admin-token-0123456789",
                reserved.broadcaster_token.as_str(),
                ephemeral.broadcaster_token.as_str(),
            ] {
                let response = list(router.clone(), Some(credential)).await;
                assert_eq!(response.status(), StatusCode::FORBIDDEN);
                assert_eq!(error_code(response).await, "invalid_admin_token");
                let response = delete(router.clone(), &reserved.event_id, Some(credential)).await;
                assert_eq!(response.status(), StatusCode::FORBIDDEN);
                assert_eq!(error_code(response).await, "invalid_admin_token");
            }

            // A rejected credential gets the same answer for an absent item,
            // so it cannot probe for an identifier.
            let response = delete(
                router.clone(),
                "no-such-item",
                Some("wrong-admin-token-0123456789"),
            )
            .await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);

            assert_eq!(reserved_row_count(&state_file), 1);
            let response = get(
                router,
                format!("/v1/liveitems/{}/remoteValue", reserved.event_id),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
        }

        #[tokio::test]
        async fn both_routes_answer_404_with_no_admin_token_configured() {
            let dir = tempfile::tempdir().expect("tempdir");
            let state_file = dir.path().join("reserved-items.sqlite3");
            let router = app(RelayState::new(AppConfig {
                state_file: state_file.clone(),
                ..AppConfig::for_tests()
            }));
            let ephemeral = create_event(router.clone()).await;

            let response = list(router.clone(), Some(ADMIN_TOKEN)).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert_eq!(error_code(response).await, "reserved_items_disabled");
            let response = delete(router.clone(), &ephemeral.event_id, Some(ADMIN_TOKEN)).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert_eq!(error_code(response).await, "reserved_items_disabled");

            let response = get(
                router,
                format!("/v1/liveitems/{}/remoteValue", ephemeral.event_id),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert!(!state_file.exists(), "a disabled feature opens no file");
        }

        #[tokio::test]
        async fn a_subscriber_to_a_deleted_item_stops() {
            let (router, _dir, _state_file) = reserved_app();
            let reserved = reserve_ok(router.clone(), None).await;
            let ephemeral = create_event(router.clone()).await;

            let response = get(
                router.clone(),
                format!("/v1/liveitems/{}/events", reserved.event_id),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let mut body = response.into_body();
            let response = get(
                router.clone(),
                format!("/v1/liveitems/{}/events", ephemeral.event_id),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let mut ephemeral_body = response.into_body();

            let response = delete(router.clone(), &reserved.event_id, Some(ADMIN_TOKEN)).await;
            assert_eq!(response.status(), StatusCode::NO_CONTENT);

            // The stream ends. A keep-alive comment can come first.
            let ended = timeout(Duration::from_secs(2), async {
                while let Some(frame) = body.frame().await {
                    let frame = frame.expect("frame");
                    if let Some(data) = frame.data_ref() {
                        assert!(
                            !String::from_utf8_lossy(data).contains("remoteValue"),
                            "a deleted item sent an update"
                        );
                    }
                }
            })
            .await;
            assert!(
                ended.is_ok(),
                "the SSE stream of a deleted item did not stop"
            );

            // The stream of another event stays open and gets its updates.
            let response = publish(
                router,
                &ephemeral.event_id,
                Some(&ephemeral.broadcaster_token),
                json!({ "title": "still live" }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let chunk = next_sse_chunk(&mut ephemeral_body).await;
            assert!(chunk.contains("still live"), "{chunk}");
        }

        #[tokio::test]
        async fn delete_store_failure_returns_500_and_changes_nothing() {
            let (router, _dir, state_file) = reserved_app();
            let reserved = reserve_ok(router.clone(), Some("station")).await;
            rusqlite::Connection::open(&state_file)
                .expect("open state file")
                .execute("DROP TABLE live_items", [])
                .expect("drop table");

            let response = delete(router.clone(), &reserved.event_id, Some(ADMIN_TOKEN)).await;

            assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
            let body: Value = read_json(response).await;
            assert_eq!(body, json!({ "error": "store_unavailable" }));
            // The file write comes first. It failed, so the item stays live.
            let response = publish(
                router.clone(),
                &reserved.event_id,
                Some(&reserved.broadcaster_token),
                json!({ "title": "still here" }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(listed_ids(&list_ok(router).await), vec![reserved.event_id]);
        }
    }
}
