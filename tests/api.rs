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

/// Publishes with explicit `Listener-Delay-Secs` header values (ADR 0004).
///
/// An empty `listener_delay_values` sends no header. More than one value
/// sends the header more than once.
async fn publish_with_listener_delay(
    router: axum::Router,
    event_id: &str,
    token: Option<&str>,
    listener_delay_values: &[&str],
    body: Value,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!("/v1/liveitems/{event_id}/metadata"))
        .header(header::CONTENT_TYPE, "application/json");

    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    for value in listener_delay_values {
        builder = builder.header("Listener-Delay-Secs", *value);
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

/// Tests for the `Listener-Delay-Secs` header of ADR 0004 (task 001). No
/// test in this group checks timing. ADR 0004 task 002 and task 003 add the
/// listener timeline that this header feeds.
mod listener_delay_header {
    use super::*;

    #[tokio::test]
    async fn a_publish_with_no_header_is_accepted_with_delay_zero() {
        let (router, state, _clock) = test_app_with(AppConfig::for_tests());
        let created = create_event(router.clone()).await;

        let response = publish(
            router,
            &created.event_id,
            Some(&created.broadcaster_token),
            json!({"title": "Live"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        assert_eq!(
            state
                .listener_delay_secs(&created.event_id)
                .await
                .expect("delay"),
            0
        );
    }

    #[tokio::test]
    async fn a_publish_with_a_valid_header_is_accepted_and_stores_the_delay() {
        let (router, state, _clock) = test_app_with(AppConfig::for_tests());
        let created = create_event(router.clone()).await;

        let response = publish_with_listener_delay(
            router,
            &created.event_id,
            Some(&created.broadcaster_token),
            &["30"],
            json!({"title": "Live"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        assert_eq!(
            state
                .listener_delay_secs(&created.event_id)
                .await
                .expect("delay"),
            30
        );
    }

    #[tokio::test]
    async fn an_invalid_header_value_returns_400_invalid_listener_delay() {
        let (router, _state, _clock) = test_app_with(AppConfig::for_tests());
        let created = create_event(router.clone()).await;

        for value in ["-1", "1.5", "abc", "", "301"] {
            let response = publish_with_listener_delay(
                router.clone(),
                &created.event_id,
                Some(&created.broadcaster_token),
                &[value],
                json!({"title": "Live"}),
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "value {value:?} must be rejected"
            );
            let body: Value = read_json(response).await;
            assert_eq!(body["error"], "invalid_listener_delay", "value {value:?}");
        }
    }

    #[tokio::test]
    async fn more_than_one_header_returns_400_invalid_listener_delay() {
        let (router, _state, _clock) = test_app_with(AppConfig::for_tests());
        let created = create_event(router.clone()).await;

        let response = publish_with_listener_delay(
            router,
            &created.event_id,
            Some(&created.broadcaster_token),
            &["5", "6"],
            json!({"title": "Live"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value = read_json(response).await;
        assert_eq!(body["error"], "invalid_listener_delay");
    }

    #[tokio::test]
    async fn a_bad_header_does_not_use_a_publish_rate_limit_slot() {
        let (router, _state, _clock) = test_app_with(AppConfig {
            max_publishes_per_event_per_sec: 1,
            ..AppConfig::for_tests()
        });
        let created = create_event(router.clone()).await;

        let response = publish_with_listener_delay(
            router.clone(),
            &created.event_id,
            Some(&created.broadcaster_token),
            &["abc"],
            json!({"title": "Live"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // The rate limit allows one publish per second. The bad header above
        // did not use that slot, so this publish still succeeds.
        let response = publish(
            router,
            &created.event_id,
            Some(&created.broadcaster_token),
            json!({"title": "Live"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn max_listener_delay_secs_sets_the_accepted_range() {
        let (router, state, _clock) = test_app_with(AppConfig {
            max_listener_delay_secs: 10,
            ..AppConfig::for_tests()
        });
        let created = create_event(router.clone()).await;

        let response = publish_with_listener_delay(
            router.clone(),
            &created.event_id,
            Some(&created.broadcaster_token),
            &["11"],
            json!({"title": "Live"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value = read_json(response).await;
        assert_eq!(body["error"], "invalid_listener_delay");

        let response = publish_with_listener_delay(
            router,
            &created.event_id,
            Some(&created.broadcaster_token),
            &["10"],
            json!({"title": "Live"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            state
                .listener_delay_secs(&created.event_id)
                .await
                .expect("delay"),
            10
        );
    }

    #[tokio::test]
    async fn the_header_name_is_matched_with_no_case() {
        let (router, state, _clock) = test_app_with(AppConfig::for_tests());
        let created = create_event(router.clone()).await;

        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/liveitems/{}/metadata", created.event_id))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {}", created.broadcaster_token),
                    )
                    .header("LISTENER-DELAY-SECS", "15")
                    .body(Body::from(json!({"title": "Live"}).to_string()))
                    .expect("publish request"),
            )
            .await
            .expect("publish response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            state
                .listener_delay_secs(&created.event_id)
                .await
                .expect("delay"),
            15
        );
    }
}

/// Tests for the listener timeline of ADR 0004 (task 002). Socket.IO and
/// `GET /remoteValue` read the listener value. SSE and `GET /metadata`
/// still read `latest`, and stay instant.
mod listener_timeline {
    use super::*;

    async fn remote_value(router: axum::Router, event_id: &str) -> Value {
        let response = router
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/liveitems/{event_id}/remoteValue"))
                    .body(Body::empty())
                    .expect("remoteValue request"),
            )
            .await
            .expect("remoteValue response");
        read_json(response).await
    }

    async fn latest_metadata_value(router: axum::Router, event_id: &str) -> Value {
        let response = router
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/liveitems/{event_id}/metadata"))
                    .body(Body::empty())
                    .expect("metadata request"),
            )
            .await
            .expect("metadata response");
        let latest: LatestMetadataResponse = read_json(response).await;
        latest.metadata
    }

    #[tokio::test]
    async fn a_publish_with_no_header_changes_the_listener_value_at_once() {
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

        // No call to `release_listener_updates`: the fast path alone makes
        // the new payload visible at once, as before ADR 0004.
        assert_eq!(
            remote_value(router, &created.event_id).await,
            json!({"title": "Live"})
        );
    }

    #[tokio::test]
    async fn a_delayed_publish_reaches_the_instant_timeline_at_once_and_the_listener_timeline_after_the_delay()
     {
        let (router, state, clock) = test_app_with(AppConfig::for_tests());
        let created = create_event(router.clone()).await;

        // T = 1: an undelayed publish gives the listener timeline a
        // previous value to keep until the release.
        let response = publish(
            router.clone(),
            &created.event_id,
            Some(&created.broadcaster_token),
            json!({"title": "First"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // T = 2: a publish with a 30-second delay.
        clock.store(2, Ordering::SeqCst);
        let response = publish_with_listener_delay(
            router.clone(),
            &created.event_id,
            Some(&created.broadcaster_token),
            &["30"],
            json!({"title": "Second"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // The instant timeline, `GET /metadata`, gets the new value at once.
        assert_eq!(
            latest_metadata_value(router.clone(), &created.event_id).await,
            json!({"title": "Second"})
        );

        // T = 31 (T + 29): the listener timeline still gives the previous
        // value. The sweep releases nothing.
        clock.store(31, Ordering::SeqCst);
        assert_eq!(state.release_listener_updates().await, 0);
        assert_eq!(
            remote_value(router.clone(), &created.event_id).await,
            json!({"title": "First"})
        );

        // T = 32 (T + 30): the sweep releases the update onto the listener
        // timeline.
        clock.store(32, Ordering::SeqCst);
        assert_eq!(state.release_listener_updates().await, 1);
        assert_eq!(
            remote_value(router, &created.event_id).await,
            json!({"title": "Second"})
        );
    }

    #[tokio::test]
    async fn a_short_delay_after_a_long_delay_is_released_in_the_order_of_the_instant_timeline() {
        let (router, state, clock) = test_app_with(AppConfig::for_tests());
        let created = create_event(router.clone()).await;

        // T = 1: a publish with a 30-second delay. Nothing has reached the
        // listener timeline yet, so `GET /remoteValue` still gives `{}`.
        let response = publish_with_listener_delay(
            router.clone(),
            &created.event_id,
            Some(&created.broadcaster_token),
            &["30"],
            json!({"title": "A"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // T = 2 (T + 1): a publish with no delay. Its release still waits
        // for the update ahead of it on the listener timeline.
        clock.store(2, Ordering::SeqCst);
        let response = publish(
            router.clone(),
            &created.event_id,
            Some(&created.broadcaster_token),
            json!({"title": "B"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        assert_eq!(state.release_listener_updates().await, 0);
        assert_eq!(
            remote_value(router.clone(), &created.event_id).await,
            json!({}),
            "the second update must not release at T + 1"
        );

        // T = 31 (the first publish's T + 30): both updates release
        // together, in order, so the second update's payload is the final
        // listener value.
        clock.store(31, Ordering::SeqCst);
        assert_eq!(state.release_listener_updates().await, 2);
        assert_eq!(
            remote_value(router, &created.event_id).await,
            json!({"title": "B"})
        );
    }

    #[tokio::test]
    async fn a_publish_with_no_header_after_a_full_release_is_at_once_again() {
        let (router, state, clock) = test_app_with(AppConfig::for_tests());
        let created = create_event(router.clone()).await;

        let response = publish_with_listener_delay(
            router.clone(),
            &created.event_id,
            Some(&created.broadcaster_token),
            &["30"],
            json!({"title": "Delayed"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        clock.store(31, Ordering::SeqCst);
        assert_eq!(state.release_listener_updates().await, 1);
        assert_eq!(
            remote_value(router.clone(), &created.event_id).await,
            json!({"title": "Delayed"})
        );

        // With no pending update left, a new publish with no header is at
        // once again, with no sweep.
        clock.store(32, Ordering::SeqCst);
        let response = publish(
            router.clone(),
            &created.event_id,
            Some(&created.broadcaster_token),
            json!({"title": "Live"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            remote_value(router, &created.event_id).await,
            json!({"title": "Live"})
        );
    }

    #[tokio::test]
    async fn a_full_pending_list_has_the_new_update_replace_the_newest_pending_update() {
        let (router, state, clock) = test_app_with(AppConfig {
            max_pending_listener_updates: 2,
            ..AppConfig::for_tests()
        });
        let created = create_event(router.clone()).await;

        for (time, title) in [(1_u64, "First"), (2, "Second"), (3, "Third")] {
            clock.store(time, Ordering::SeqCst);
            let response = publish_with_listener_delay(
                router.clone(),
                &created.event_id,
                Some(&created.broadcaster_token),
                &["30"],
                json!({"title": title}),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
        }

        // The pending list holds at most 2 updates: "First" and "Third".
        // "Second" was replaced before it ever reached the listener
        // timeline.
        clock.store(33, Ordering::SeqCst);
        assert_eq!(state.release_listener_updates().await, 2);
        assert_eq!(
            remote_value(router, &created.event_id).await,
            json!({"title": "Third"})
        );
    }

    #[tokio::test]
    async fn a_client_that_reads_the_listener_value_while_an_update_waits_gets_the_old_value() {
        let (router, state, clock) = test_app_with(AppConfig::for_tests());
        let created = create_event(router.clone()).await;

        let response = publish(
            router.clone(),
            &created.event_id,
            Some(&created.broadcaster_token),
            json!({"title": "Old"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        clock.store(2, Ordering::SeqCst);
        let response = publish_with_listener_delay(
            router,
            &created.event_id,
            Some(&created.broadcaster_token),
            &["30"],
            json!({"title": "New"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // `register_socket_namespaces` reads the initial emit through
        // `RelayState::listener_value`, the same function this test calls.
        // A client that connects now, before the release, gets "Old", not
        // the newest published payload.
        assert_eq!(
            state
                .listener_value(&created.event_id)
                .await
                .expect("listener value"),
            json!({"title": "Old"})
        );
    }

    /// ADR 0004 §The Lease And The `404`: a lease expiry joins the
    /// listener timeline with the delay of the last publish, so `{}`
    /// arrives on the listener timeline after the last block, not before
    /// it (task 003).
    #[tokio::test]
    async fn a_lease_expiry_joins_the_listener_timeline_with_the_delay_of_the_last_publish() {
        let (router, state, clock) = test_app_with(AppConfig::for_tests());
        let created = create_event(router.clone()).await;

        // T = 1: a publish with a 30-second delay.
        let response = publish_with_listener_delay(
            router.clone(),
            &created.event_id,
            Some(&created.broadcaster_token),
            &["30"],
            json!({"title": "Live"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // T = 31 (T + 30): the sweep releases the payload.
        clock.store(31, Ordering::SeqCst);
        assert_eq!(state.release_listener_updates().await, 1);
        assert_eq!(
            remote_value(router.clone(), &created.event_id).await,
            json!({"title": "Live"})
        );

        // T = 91 (T + 90, the default lease): the lease expires. The
        // instant side, `GET /metadata`, gives `404` at once.
        clock.store(91, Ordering::SeqCst);
        assert_eq!(state.expire_leases().await, 1);
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/liveitems/{}/metadata", created.event_id))
                    .body(Body::empty())
                    .expect("metadata request"),
            )
            .await
            .expect("metadata response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        // T = 120 (T + 119): the listener timeline still gives the payload.
        clock.store(120, Ordering::SeqCst);
        assert_eq!(state.release_listener_updates().await, 0);
        assert_eq!(
            remote_value(router.clone(), &created.event_id).await,
            json!({"title": "Live"})
        );

        // T = 121 (T + 120): `{}` reaches the listener timeline.
        clock.store(121, Ordering::SeqCst);
        assert_eq!(state.release_listener_updates().await, 1);
        assert_eq!(remote_value(router, &created.event_id).await, json!({}));
    }

    #[tokio::test]
    async fn a_delay_longer_than_the_lease_keeps_the_order_through_the_expiry() {
        let (router, state, clock) = test_app_with(AppConfig::for_tests());
        let created = create_event(router.clone()).await;

        // T = 1: a publish with a 120-second delay.
        let response = publish_with_listener_delay(
            router.clone(),
            &created.event_id,
            Some(&created.broadcaster_token),
            &["120"],
            json!({"title": "First"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // T = 6 (T + 5): a second publish, also with a 120-second delay.
        clock.store(6, Ordering::SeqCst);
        let response = publish_with_listener_delay(
            router.clone(),
            &created.event_id,
            Some(&created.broadcaster_token),
            &["120"],
            json!({"title": "Second"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // T = 96 (T + 95): the lease expires before either update
        // releases. The order rule holds the `{}` behind both.
        clock.store(96, Ordering::SeqCst);
        assert_eq!(state.expire_leases().await, 1);
        assert_eq!(state.release_listener_updates().await, 0);

        // T = 121 (T + 120): the first payload releases.
        clock.store(121, Ordering::SeqCst);
        assert_eq!(state.release_listener_updates().await, 1);
        assert_eq!(
            remote_value(router.clone(), &created.event_id).await,
            json!({"title": "First"})
        );

        // T = 126 (T + 125): the second payload releases.
        clock.store(126, Ordering::SeqCst);
        assert_eq!(state.release_listener_updates().await, 1);
        assert_eq!(
            remote_value(router.clone(), &created.event_id).await,
            json!({"title": "Second"})
        );

        // T = 216 (T + 215): `{}` releases last.
        clock.store(216, Ordering::SeqCst);
        assert_eq!(state.release_listener_updates().await, 1);
        assert_eq!(remote_value(router, &created.event_id).await, json!({}));
    }

    #[tokio::test]
    async fn a_lease_expiry_with_no_delay_gives_the_empty_object_at_once() {
        let (router, state, clock) = test_app_with(AppConfig::for_tests());
        let created = create_event(router.clone()).await;

        let response = publish(
            router.clone(),
            &created.event_id,
            Some(&created.broadcaster_token),
            json!({"title": "Live"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        clock.store(91, Ordering::SeqCst); // T + 90, the default lease
        assert_eq!(state.expire_leases().await, 1);

        // No call to `release_listener_updates`: a delay of 0 with no
        // pending update reaches the listener timeline at once, exactly as
        // before this ADR.
        assert_eq!(remote_value(router, &created.event_id).await, json!({}));
    }
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

        /// ADR 0004 §The Pending Updates: a delete removes the pending
        /// listener updates of the event, so no later sweep sends one
        /// (task 003).
        #[tokio::test]
        async fn a_delete_with_a_pending_update_sends_no_release() {
            let dir = tempfile::tempdir().expect("tempdir");
            let state_file = dir.path().join("reserved-items.sqlite3");
            let clock = Arc::new(AtomicU64::new(1));
            let state = reserved_state_at(&state_file, &clock).expect("open state");
            let router = app(state.clone());
            let reserved = reserve_ok(router.clone(), None).await;

            let response = publish_with_listener_delay(
                router.clone(),
                &reserved.event_id,
                Some(&reserved.broadcaster_token),
                &["30"],
                json!({"title": "Pending"}),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);

            let response = delete(router, &reserved.event_id, Some(ADMIN_TOKEN)).await;
            assert_eq!(response.status(), StatusCode::NO_CONTENT);

            clock.store(100, Ordering::SeqCst); // past the release time
            assert_eq!(
                state.release_listener_updates().await,
                0,
                "a deleted event sends no pending update"
            );
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

    /// One guard for each invariant of ADR 0001 §Invariants (task 005).
    ///
    /// The constant-time invariant has its guard in the unit tests of
    /// `src/lib.rs`, because the comparison functions are private.
    mod adr_0001_invariants {
        use sha2::{Digest, Sha256};

        use super::*;

        const WRONG_ADMIN_TOKEN: &str = "wrong-admin-token-0123456789";

        /// Returns the name and the bytes of each file in `dir`, sorted by
        /// name.
        fn directory_files(dir: &Path) -> Vec<(String, Vec<u8>)> {
            let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
                .expect("read state directory")
                .map(|entry| {
                    let path = entry.expect("entry").path();
                    let name = path
                        .file_name()
                        .expect("name")
                        .to_string_lossy()
                        .into_owned();
                    (name, std::fs::read(&path).expect("read file"))
                })
                .collect();
            files.sort();
            files
        }

        fn contains(haystack: &[u8], needle: &[u8]) -> bool {
            haystack
                .windows(needle.len())
                .any(|window| window == needle)
        }

        async fn send(
            router: axum::Router,
            method: &str,
            uri: String,
            token: Option<&str>,
            body: Option<Value>,
        ) -> axum::response::Response {
            let mut builder = Request::builder().method(method).uri(uri);
            if let Some(token) = token {
                builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
            }
            let body = match body {
                Some(body) => {
                    builder = builder.header(header::CONTENT_TYPE, "application/json");
                    Body::from(body.to_string())
                }
                None => Body::empty(),
            };
            router
                .oneshot(builder.body(body).expect("request"))
                .await
                .expect("response")
        }

        /// Asserts that each route of `event_id` answers `404` with the
        /// error code `event_not_found`.
        async fn assert_event_does_not_exist(router: &axum::Router, event_id: &str, token: &str) {
            let requests = [
                ("GET", "metadata", None),
                ("GET", "remoteValue", None),
                ("GET", "events", None),
                ("POST", "metadata", Some(json!({ "title": "x" }))),
                ("POST", "keepalive", None),
            ];
            for (method, route, body) in requests {
                let response = send(
                    router.clone(),
                    method,
                    format!("/v1/liveitems/{event_id}/{route}"),
                    Some(token),
                    body,
                )
                .await;
                assert_eq!(
                    response.status(),
                    StatusCode::NOT_FOUND,
                    "{method} {route} of {event_id}"
                );
                assert_eq!(
                    error_code(response).await,
                    "event_not_found",
                    "{method} {route} of {event_id}"
                );
            }
        }

        /// Asserts that `event_id` exists and has no snapshot. No route
        /// answers `event_not_found`.
        async fn assert_event_exists_with_no_snapshot(
            router: &axum::Router,
            event_id: &str,
            token: &str,
        ) {
            let response = get(
                router.clone(),
                format!("/v1/liveitems/{event_id}/remoteValue"),
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "remoteValue of {event_id}"
            );
            let value: Value = read_json(response).await;
            assert_eq!(value, json!({}), "remoteValue of {event_id}");

            // ADR 0002: an event with no snapshot gives `404` on the metadata
            // read, with its own error code. The event exists.
            let response = get(router.clone(), format!("/v1/liveitems/{event_id}/metadata")).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert_eq!(error_code(response).await, "metadata_not_found");

            let response = get(router.clone(), format!("/v1/liveitems/{event_id}/events")).await;
            assert_eq!(response.status(), StatusCode::OK, "events of {event_id}");

            let response = keepalive(router.clone(), event_id, Some(token)).await;
            assert_eq!(response.status(), StatusCode::CONFLICT);
            assert_eq!(error_code(response).await, "lease_expired");
        }

        // Invariant: ephemeral behavior does not change. Every test before
        // `mod reserved` passes unchanged. This test runs the ephemeral life
        // cycle on a relay that has a state file.
        #[tokio::test]
        async fn ephemeral_behavior_does_not_change_with_a_state_file() {
            let dir = tempfile::tempdir().expect("tempdir");
            let state_file = dir.path().join("reserved-items.sqlite3");
            let clock = Arc::new(AtomicU64::new(100));
            let state = reserved_state_at(&state_file, &clock).expect("open state");
            let router = app(state.clone());
            let before = directory_files(dir.path());

            let response = send(
                router.clone(),
                "POST",
                "/v1/liveitems".to_string(),
                None,
                None,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let body: Value = read_json(response).await;
            let mut keys: Vec<&str> = body
                .as_object()
                .expect("object")
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(
                keys,
                [
                    "broadcaster_token",
                    "event_id",
                    "events_url",
                    "metadata_url",
                    "remote_value_url",
                    "socket_io_url",
                ]
            );
            let event_id = body["event_id"].as_str().expect("event_id").to_string();
            let token = body["broadcaster_token"]
                .as_str()
                .expect("token")
                .to_string();

            let response = publish(
                router.clone(),
                &event_id,
                Some(&token),
                json!({ "title": "ephemeral show" }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let response = get(
                router.clone(),
                format!("/v1/liveitems/{event_id}/remoteValue"),
            )
            .await;
            let value: Value = read_json(response).await;
            assert_eq!(value, json!({ "title": "ephemeral show" }));

            // An ephemeral item writes nothing to the state directory.
            assert_eq!(directory_files(dir.path()), before);

            // A restart loses the ephemeral item.
            let restarted = app(reserved_state_at(&state_file, &clock).expect("restart"));
            assert_event_does_not_exist(&restarted, &event_id, &token).await;

            // The reaper removes the ephemeral item after the idle TTL.
            clock.store(111, Ordering::SeqCst);
            assert_eq!(state.cleanup_expired().await, 1);
            assert_event_does_not_exist(&router, &event_id, &token).await;
        }

        // Invariant: no payload, snapshot, or replay buffer reaches disk.
        // Packet step 2: the state file after a publish holds no payload.
        #[tokio::test]
        async fn no_payload_snapshot_or_replay_buffer_reaches_disk() {
            let dir = tempfile::tempdir().expect("tempdir");
            let state_file = dir.path().join("reserved-items.sqlite3");
            let clock = Arc::new(AtomicU64::new(100));
            let state = reserved_state_at(&state_file, &clock).expect("open state");
            let router = app(state.clone());
            let reserved = reserve_ok(router.clone(), Some("station")).await;
            let ephemeral = create_event(router.clone()).await;
            let after_reserve = directory_files(dir.path());

            let markers = ["payload-marker-a81f", "payload-marker-b27e"];
            let field = "payload_field_key_c93d";
            for (step, marker) in markers.iter().enumerate() {
                clock.store(101 + step as u64, Ordering::SeqCst);
                for (event_id, token) in [
                    (&reserved.event_id, &reserved.broadcaster_token),
                    (&ephemeral.event_id, &ephemeral.broadcaster_token),
                ] {
                    let response = publish(
                        router.clone(),
                        event_id,
                        Some(token),
                        json!({
                            "title": marker,
                            field: marker,
                            "value": { "destinations": [{ "name": marker, "split": "100" }] },
                        }),
                    )
                    .await;
                    assert_eq!(response.status(), StatusCode::OK);
                }
            }
            let response = keepalive(
                router.clone(),
                &reserved.event_id,
                Some(&reserved.broadcaster_token),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            // A lease expiry adds a `{}` update to the replay buffer.
            clock.store(200, Ordering::SeqCst);
            assert!(state.expire_leases().await >= 1);
            drop(router);
            drop(state);

            // A publish, a keepalive, and a lease expiry change no byte in
            // the state directory, and add no file to it.
            let after_publish = directory_files(dir.path());
            assert_eq!(after_publish, after_reserve);
            for (name, bytes) in &after_publish {
                for needle in markers.iter().chain([&field]) {
                    assert!(
                        !contains(bytes, needle.as_bytes()),
                        "{name} holds the payload text {needle}"
                    );
                }
            }

            // The schema holds no place for a payload.
            let connection = rusqlite::Connection::open(&state_file).expect("open state file");
            let mut statement = connection
                .prepare("SELECT type, name FROM sqlite_schema ORDER BY type, name")
                .expect("prepare");
            let objects: Vec<(String, String)> = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .expect("query")
                .collect::<Result<_, _>>()
                .expect("objects");
            let tables: Vec<&str> = objects
                .iter()
                .filter(|(kind, _)| kind != "index")
                .map(|(_, name)| name.as_str())
                .collect();
            assert_eq!(tables, ["live_items", "schema_version"]);
            assert!(
                objects
                    .iter()
                    .filter(|(kind, _)| kind == "index")
                    .all(|(_, name)| name.starts_with("sqlite_autoindex_live_items")),
                "{objects:?}"
            );
            assert_eq!(
                table_columns(&state_file, "live_items"),
                ["event_id", "token_hash", "label", "created_at", "class"]
            );
            assert_eq!(table_columns(&state_file, "schema_version"), ["version"]);
            assert_eq!(reserved_row_count(&state_file), 1);
        }

        // Invariant: the stored value for a token is a hash. The token itself
        // is never stored.
        #[tokio::test]
        async fn the_state_file_holds_the_token_hash_and_never_the_token() {
            let (router, dir, state_file) = reserved_app();
            let reserved = reserve_ok(router.clone(), Some("station")).await;
            drop(router);

            let stored: Vec<u8> = rusqlite::Connection::open(&state_file)
                .expect("open state file")
                .query_row(
                    "SELECT token_hash FROM live_items WHERE event_id = ?1",
                    [&reserved.event_id],
                    |row| row.get(0),
                )
                .expect("token hash");
            let expected: [u8; 32] = Sha256::digest(reserved.broadcaster_token.as_bytes()).into();
            assert_eq!(stored, expected);
            assert_ne!(stored, reserved.broadcaster_token.as_bytes());

            // The admin token and its hash stay in memory only.
            let admin_hash: [u8; 32] = Sha256::digest(ADMIN_TOKEN.as_bytes()).into();
            for (name, bytes) in directory_files(dir.path()) {
                assert!(
                    !contains(&bytes, reserved.broadcaster_token.as_bytes()),
                    "{name} holds the broadcaster token"
                );
                assert!(
                    !contains(&bytes, ADMIN_TOKEN.as_bytes()),
                    "{name} holds the admin token"
                );
                assert!(
                    !contains(&bytes, &admin_hash),
                    "{name} holds the admin token hash"
                );
            }
        }

        // Invariant: a route that writes to durable storage needs the admin
        // credential. Only the reserve route and the delete route write to
        // the state file.
        #[tokio::test]
        async fn each_durable_write_needs_the_admin_credential() {
            let (router, dir, state_file) = reserved_app();
            let reserved = reserve_ok(router.clone(), Some("station")).await;
            let ephemeral = create_event(router.clone()).await;
            let before = directory_files(dir.path());

            let credentials = [
                (None, StatusCode::UNAUTHORIZED),
                (Some(WRONG_ADMIN_TOKEN), StatusCode::FORBIDDEN),
                (
                    Some(reserved.broadcaster_token.as_str()),
                    StatusCode::FORBIDDEN,
                ),
                (
                    Some(ephemeral.broadcaster_token.as_str()),
                    StatusCode::FORBIDDEN,
                ),
            ];
            for (credential, status) in credentials {
                let response = send(
                    router.clone(),
                    "POST",
                    "/v1/liveitems/reserved".to_string(),
                    credential,
                    Some(json!({ "label": "intruder" })),
                )
                .await;
                assert_eq!(response.status(), status, "reserve with {credential:?}");
                let response = send(
                    router.clone(),
                    "DELETE",
                    format!("/v1/liveitems/reserved/{}", reserved.event_id),
                    credential,
                    None,
                )
                .await;
                assert_eq!(response.status(), status, "delete with {credential:?}");
            }

            // The public routes write nothing to the state directory.
            let response = publish(
                router.clone(),
                &reserved.event_id,
                Some(&reserved.broadcaster_token),
                json!({ "title": "on air" }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            create_event(router.clone()).await;
            assert_eq!(directory_files(dir.path()), before);
            assert_eq!(reserved_row_count(&state_file), 1);

            // With the admin credential, both routes write to the file.
            reserve_ok(router.clone(), Some("second")).await;
            assert_eq!(reserved_row_count(&state_file), 2);
            let response = send(
                router,
                "DELETE",
                format!("/v1/liveitems/reserved/{}", reserved.event_id),
                Some(ADMIN_TOKEN),
                None,
            )
            .await;
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
            assert_eq!(reserved_row_count(&state_file), 1);
        }

        // Invariant: the reaper never removes a reserved item, in each state
        // that a reserved item can have.
        #[tokio::test]
        async fn the_reaper_never_removes_a_reserved_item() {
            let dir = tempfile::tempdir().expect("tempdir");
            let state_file = dir.path().join("reserved-items.sqlite3");
            let restored = reserve_publish_and_stop(&state_file, "before restart").await;

            let clock = Arc::new(AtomicU64::new(5_000));
            let state = reserved_state_at(&state_file, &clock).expect("restore");
            let router = app(state.clone());
            let never_published = reserve_ok(router.clone(), Some("never published")).await;
            let on_air = reserve_ok(router.clone(), Some("on air")).await;
            let response = publish(
                router.clone(),
                &on_air.event_id,
                Some(&on_air.broadcaster_token),
                json!({ "title": "on air" }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let ephemeral = create_event(router.clone()).await;

            // Ten years with no activity. The TTL is 10 seconds.
            clock.store(5_000 + 10 * 365 * 24 * 60 * 60, Ordering::SeqCst);
            state.expire_leases().await;
            assert_eq!(state.cleanup_expired().await, 1);
            assert_eq!(state.cleanup_expired().await, 0);

            for reserved in [&restored, &never_published, &on_air] {
                assert_event_exists_with_no_snapshot(
                    &router,
                    &reserved.event_id,
                    &reserved.broadcaster_token,
                )
                .await;
            }
            assert_event_does_not_exist(&router, &ephemeral.event_id, &ephemeral.broadcaster_token)
                .await;
            assert_eq!(reserved_row_count(&state_file), 3);
        }

        // Invariant: a `404` with the error code `event_not_found` still means
        // that the event does not exist, for both classes. An event that
        // exists never gets that answer.
        #[tokio::test]
        async fn event_not_found_means_the_event_does_not_exist_for_both_classes() {
            let dir = tempfile::tempdir().expect("tempdir");
            let state_file = dir.path().join("reserved-items.sqlite3");
            let clock = Arc::new(AtomicU64::new(100));
            let state = reserved_state_at(&state_file, &clock).expect("open state");
            let router = app(state.clone());
            let any_token = "any-token";

            // An identifier that never existed.
            assert_event_does_not_exist(&router, "no-such-event", any_token).await;

            // Both classes exist before their first publish.
            let reserved = reserve_ok(router.clone(), Some("station")).await;
            let deleted = reserve_ok(router.clone(), Some("deleted")).await;
            let ephemeral = create_event(router.clone()).await;
            for (event_id, token) in [
                (&reserved.event_id, &reserved.broadcaster_token),
                (&deleted.event_id, &deleted.broadcaster_token),
                (&ephemeral.event_id, &ephemeral.broadcaster_token),
            ] {
                assert_event_exists_with_no_snapshot(&router, event_id, token).await;
            }

            // A deleted reserved item does not exist.
            let response = send(
                router.clone(),
                "DELETE",
                format!("/v1/liveitems/reserved/{}", deleted.event_id),
                Some(ADMIN_TOKEN),
                None,
            )
            .await;
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
            assert_event_does_not_exist(&router, &deleted.event_id, &deleted.broadcaster_token)
                .await;

            // A reaped ephemeral item does not exist. The reserved item does.
            clock.store(111, Ordering::SeqCst);
            assert_eq!(state.cleanup_expired().await, 1);
            assert_event_does_not_exist(&router, &ephemeral.event_id, &ephemeral.broadcaster_token)
                .await;
            assert_event_exists_with_no_snapshot(
                &router,
                &reserved.event_id,
                &reserved.broadcaster_token,
            )
            .await;

            // After a restart, the reserved item exists with no snapshot, and
            // the deleted item stays absent.
            let restarted = app(reserved_state_at(&state_file, &clock).expect("restart"));
            assert_event_exists_with_no_snapshot(
                &restarted,
                &reserved.event_id,
                &reserved.broadcaster_token,
            )
            .await;
            assert_event_does_not_exist(&restarted, &deleted.event_id, &deleted.broadcaster_token)
                .await;
        }
    }

    /// Tests for the display state of ADR 0003, task 001: the display
    /// routes, the lease expiry rule, and the separation from the live
    /// value transports.
    mod display {
        use musicindex_live_relay::{PublishDisplayResponse, UploadArtworkResponse};

        use super::*;

        const SHA256: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

        /// Builds a relay with an admin token, a lease of 10 seconds, and a
        /// clock at 100. Keep the `TempDir` until the test ends.
        fn display_app() -> (axum::Router, RelayState, Arc<AtomicU64>, TempDir, PathBuf) {
            let dir = tempfile::tempdir().expect("tempdir");
            let state_file = dir.path().join("reserved-items.sqlite3");
            let clock = Arc::new(AtomicU64::new(100));
            let state = reserved_state_at(&state_file, &clock).expect("open state");
            (app(state.clone()), state, clock, dir, state_file)
        }

        fn track(artwork: Value) -> Value {
            json!({ "track": { "artist": "Artist", "title": "Title", "artwork": artwork } })
        }

        async fn post_display(
            router: axum::Router,
            event_id: &str,
            authorization: Option<&str>,
            body: impl Into<Body>,
        ) -> axum::response::Response {
            let mut builder = Request::builder()
                .method("POST")
                .uri(format!("/v1/liveitems/{event_id}/display"))
                .header(header::CONTENT_TYPE, "application/json");
            if let Some(authorization) = authorization {
                builder = builder.header(header::AUTHORIZATION, authorization);
            }
            router
                .oneshot(builder.body(body.into()).expect("display request"))
                .await
                .expect("display response")
        }

        /// Publishes `body` with the token and asserts `200`.
        async fn publish_display_ok(
            router: axum::Router,
            reserved: &ReserveEventResponse,
            body: &Value,
        ) -> PublishDisplayResponse {
            let response = post_display(
                router,
                &reserved.event_id,
                Some(&format!("Bearer {}", reserved.broadcaster_token)),
                body.to_string(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let published: PublishDisplayResponse = read_json(response).await;
            assert!(published.accepted);
            assert_eq!(published.event_id, reserved.event_id);
            published
        }

        async fn read_display(router: axum::Router, event_id: &str) -> Value {
            let response = get(router, format!("/v1/liveitems/{event_id}/display")).await;
            assert_eq!(response.status(), StatusCode::OK);
            read_json(response).await
        }

        async fn open_stream(
            router: axum::Router,
            uri: String,
            last_event_id: Option<&str>,
        ) -> axum::response::Response {
            let mut builder = Request::builder().uri(uri);
            if let Some(last_event_id) = last_event_id {
                builder = builder.header("Last-Event-ID", last_event_id);
            }
            router
                .oneshot(builder.body(Body::empty()).expect("stream request"))
                .await
                .expect("stream response")
        }

        async fn open_display_stream(
            router: axum::Router,
            event_id: &str,
            last_event_id: Option<&str>,
        ) -> Body {
            let response = open_stream(
                router,
                format!("/v1/liveitems/{event_id}/display/events"),
                last_event_id,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            response.into_body()
        }

        /// Returns the `data` line of an SSE chunk as JSON.
        fn chunk_data(chunk: &str) -> Value {
            let data = chunk
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .unwrap_or_else(|| panic!("no data line: {chunk}"));
            serde_json::from_str(data).expect("data is json")
        }

        /// Asserts that `body` gives no frame in a short time.
        async fn assert_no_frame(body: &mut Body) {
            let frame = timeout(Duration::from_millis(200), body.frame()).await;
            assert!(frame.is_err(), "unexpected frame: {frame:?}");
        }

        #[tokio::test]
        async fn a_publish_and_a_read_give_the_same_state() {
            let (router, _state, _clock, _dir, _) = display_app();
            let reserved = reserve_ok(router.clone(), None).await;

            assert_eq!(
                read_display(router.clone(), &reserved.event_id).await,
                json!({ "track": null })
            );

            // Task 002: a display publish names only a held image, and a
            // publish removes an upload that no state names. So each image
            // is uploaded just before the state that names it.
            let images = [jpeg_bytes(1), png_bytes(1)];
            let jpeg = sha256_of(&images[0]);
            let png = sha256_of(&images[1]);
            let states = [
                track(json!({ "sha256": jpeg, "mime": "image/jpeg" })),
                track(json!({ "sha256": png, "mime": "image/png" })),
                track(json!({ "url": "https://example.com/cover.jpg" })),
                track(json!({ "url": "http://example.com/cover.png" })),
                track(Value::Null),
                json!({ "track": null }),
            ];
            for (index, state) in states.iter().enumerate() {
                if let Some(bytes) = images.get(index) {
                    upload_ok(router.clone(), &reserved, bytes).await;
                }
                let published = publish_display_ok(router.clone(), &reserved, state).await;
                assert_eq!(published.seq, index as u64 + 1);
                assert_eq!(
                    read_display(router.clone(), &reserved.event_id).await,
                    *state
                );
            }
        }

        #[tokio::test]
        async fn a_connect_with_no_last_event_id_first_gets_the_present_state() {
            let (router, _state, _clock, _dir, _) = display_app();
            let reserved = reserve_ok(router.clone(), None).await;

            // Before any publish, the present state is null with the id 0.
            let mut body = open_display_stream(router.clone(), &reserved.event_id, None).await;
            let chunk = next_sse_chunk(&mut body).await;
            assert!(chunk.contains("event: display"), "{chunk}");
            assert!(chunk.contains("id: 0"), "{chunk}");
            assert_eq!(chunk_data(&chunk), json!({ "track": null }));

            let playing = track(json!({ "url": "https://example.com/cover.jpg" }));
            publish_display_ok(router.clone(), &reserved, &playing).await;
            let chunk = next_sse_chunk(&mut body).await;
            assert!(chunk.contains("id: 1"), "{chunk}");
            assert_eq!(chunk_data(&chunk), playing);

            // A new connection gets the present state with its id, one time.
            let mut late = open_display_stream(router.clone(), &reserved.event_id, None).await;
            let chunk = next_sse_chunk(&mut late).await;
            assert!(chunk.contains("id: 1"), "{chunk}");
            assert_eq!(chunk_data(&chunk), playing);
            assert_no_frame(&mut late).await;

            // A reconnect with that id gets nothing until the next publish.
            let mut resumed =
                open_display_stream(router.clone(), &reserved.event_id, Some("1")).await;
            assert_no_frame(&mut resumed).await;
        }

        #[tokio::test]
        async fn a_reconnect_with_an_old_higher_last_event_id_still_gets_new_states() {
            let (router, _state, _clock, _dir, _) = display_app();
            let reserved = reserve_ok(router.clone(), None).await;

            // As after a relay restart: the client sends an id that is higher
            // than the present `seq`.
            let mut body =
                open_display_stream(router.clone(), &reserved.event_id, Some("57")).await;
            assert_no_frame(&mut body).await;

            let playing = track(json!({ "url": "https://example.com/cover.jpg" }));
            publish_display_ok(router.clone(), &reserved, &playing).await;
            let chunk = next_sse_chunk(&mut body).await;
            assert!(chunk.contains("id: 1"), "{chunk}");
            assert_eq!(chunk_data(&chunk), playing);
        }

        #[tokio::test]
        async fn a_subscriber_receives_each_state_and_a_reconnect_receives_the_missed_states() {
            let (router, _state, _clock, _dir, _) = display_app();
            let reserved = reserve_ok(router.clone(), None).await;
            let mut body = open_display_stream(router.clone(), &reserved.event_id, None).await;
            // The stream first sends the present state (ADR 0003, amended
            // 2026-10-04).
            let present = next_sse_chunk(&mut body).await;
            assert!(present.contains("event: display"), "{present}");

            let states: Vec<Value> = (1..=3)
                .map(|index| track(json!({ "url": format!("https://example.com/{index}.jpg") })))
                .collect();
            for (index, state) in states.iter().enumerate() {
                publish_display_ok(router.clone(), &reserved, state).await;
                let chunk = next_sse_chunk(&mut body).await;
                assert!(chunk.contains("event: display"), "{chunk}");
                assert!(chunk.contains(&format!("id: {}", index + 1)), "{chunk}");
                assert_eq!(chunk_data(&chunk), *state);
            }

            let mut replay =
                open_display_stream(router.clone(), &reserved.event_id, Some("1")).await;
            for (index, state) in states.iter().enumerate().skip(1) {
                let chunk = next_sse_chunk(&mut replay).await;
                assert!(chunk.contains("event: display"), "{chunk}");
                assert!(chunk.contains(&format!("id: {}", index + 1)), "{chunk}");
                assert_eq!(chunk_data(&chunk), *state);
            }
            assert_no_frame(&mut replay).await;
        }

        #[tokio::test]
        async fn an_ephemeral_event_gives_409_on_each_route() {
            let (router, _state, _clock, _dir, _) = display_app();
            let ephemeral = create_event(router.clone()).await;
            let authorization = format!("Bearer {}", ephemeral.broadcaster_token);

            let response = post_display(
                router.clone(),
                &ephemeral.event_id,
                Some(&authorization),
                json!({ "track": null }).to_string(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::CONFLICT);
            assert_eq!(error_code(response).await, "event_not_reserved");

            for path in ["display", "display/events"] {
                let response = get(
                    router.clone(),
                    format!("/v1/liveitems/{}/{path}", ephemeral.event_id),
                )
                .await;
                assert_eq!(response.status(), StatusCode::CONFLICT, "{path}");
                assert_eq!(error_code(response).await, "event_not_reserved", "{path}");
            }
        }

        #[tokio::test]
        async fn a_bad_credential_gives_401_or_403_and_an_unknown_event_gives_404() {
            let (router, _state, _clock, _dir, _) = display_app();
            let reserved = reserve_ok(router.clone(), Some("first")).await;
            let other = reserve_ok(router.clone(), Some("second")).await;
            let body = json!({ "track": null }).to_string();

            let cases = [
                (None, StatusCode::UNAUTHORIZED, "missing_bearer_token"),
                (
                    Some("Token abc".to_string()),
                    StatusCode::UNAUTHORIZED,
                    "invalid_bearer_token",
                ),
                (
                    Some("Bearer".to_string()),
                    StatusCode::UNAUTHORIZED,
                    "invalid_bearer_token",
                ),
                (
                    Some("Bearer wrong".to_string()),
                    StatusCode::FORBIDDEN,
                    "invalid_token",
                ),
                (
                    Some(format!("Bearer {}", other.broadcaster_token)),
                    StatusCode::FORBIDDEN,
                    "invalid_token",
                ),
                (
                    Some(format!("Bearer {ADMIN_TOKEN}")),
                    StatusCode::FORBIDDEN,
                    "invalid_token",
                ),
            ];
            for (authorization, status, code) in cases {
                let response = post_display(
                    router.clone(),
                    &reserved.event_id,
                    authorization.as_deref(),
                    body.clone(),
                )
                .await;
                assert_eq!(response.status(), status, "{authorization:?}");
                assert_eq!(error_code(response).await, code, "{authorization:?}");
            }
            // No rejected request changed the state.
            let published =
                publish_display_ok(router.clone(), &reserved, &json!({ "track": null })).await;
            assert_eq!(published.seq, 1);

            let response = post_display(
                router.clone(),
                "missing",
                Some(&format!("Bearer {}", reserved.broadcaster_token)),
                body,
            )
            .await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert_eq!(error_code(response).await, "event_not_found");
            for path in ["display", "display/events"] {
                let response = get(router.clone(), format!("/v1/liveitems/missing/{path}")).await;
                assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
                assert_eq!(error_code(response).await, "event_not_found", "{path}");
            }
        }

        #[tokio::test]
        async fn a_body_over_8_kib_gives_413() {
            let (router, _state, _clock, _dir, _) = display_app();
            let reserved = reserve_ok(router.clone(), None).await;
            let authorization = format!("Bearer {}", reserved.broadcaster_token);

            // A valid display state of exactly `len` bytes.
            let empty_len = json!({ "track": { "artist": "", "title": "", "artwork": null } })
                .to_string()
                .len();
            let sized = |len: usize| {
                let pad = len - empty_len;
                let body = json!({
                    "track": { "artist": "", "title": "a".repeat(pad), "artwork": null }
                })
                .to_string();
                assert_eq!(body.len(), len);
                body
            };

            let response = post_display(
                router.clone(),
                &reserved.event_id,
                Some(&authorization),
                sized(8 * 1024),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);

            let response = post_display(
                router.clone(),
                &reserved.event_id,
                Some(&authorization),
                sized(8 * 1024 + 1),
            )
            .await;
            assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
            assert_eq!(error_code(response).await, "payload_too_large");

            // A request that states its length is refused before the
            // handler runs.
            let body = sized(8 * 1024 + 1);
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/v1/liveitems/{}/display", reserved.event_id))
                        .header(header::AUTHORIZATION, &authorization)
                        .header(header::CONTENT_LENGTH, body.len())
                        .body(Body::from(body))
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

            // The refused bodies did not change the state.
            let state = read_display(router, &reserved.event_id).await;
            assert_eq!(
                state["track"]["title"].as_str().map(str::len),
                Some(8 * 1024 - empty_len)
            );
        }

        #[tokio::test]
        async fn a_wrong_shape_or_a_wrong_url_gives_400() {
            let (router, _state, _clock, _dir, _) = display_app();
            let reserved = reserve_ok(router.clone(), None).await;
            let authorization = format!("Bearer {}", reserved.broadcaster_token);
            let long_url = format!("https://example.com/{}", "a".repeat(2_048 - 20));
            assert_eq!(long_url.chars().count(), 2_048);

            let wrong: Vec<String> = [
                json!([]),
                json!({}),
                json!({ "track": null, "schema": "musicindex.display/1" }),
                json!({ "track": "Title" }),
                json!({ "track": { "artist": "Artist", "title": "Title" } }),
                json!({ "track": { "title": "Title", "artwork": null } }),
                json!({ "track": { "artist": 1, "title": "Title", "artwork": null } }),
                json!({ "track": { "artist": "Artist", "title": null, "artwork": null } }),
                json!({ "track": {
                    "artist": "Artist", "title": "Title", "artwork": null, "album": "Album"
                } }),
                track(json!("https://example.com/cover.jpg")),
                track(json!({})),
                track(json!({ "url": "https://example.com/a.jpg", "sha256": SHA256 })),
                track(json!({ "url": 1 })),
                track(json!({ "sha256": SHA256 })),
                track(json!({ "sha256": SHA256, "mime": "image/gif" })),
                track(json!({ "sha256": SHA256, "mime": "IMAGE/JPEG" })),
                track(json!({ "sha256": SHA256.to_uppercase(), "mime": "image/jpeg" })),
                track(json!({ "sha256": &SHA256[1..], "mime": "image/jpeg" })),
                track(json!({ "sha256": format!("{SHA256}0"), "mime": "image/jpeg" })),
                track(json!({ "sha256": SHA256.replace('a', "g"), "mime": "image/jpeg" })),
                track(json!({ "sha256": SHA256, "mime": "image/jpeg", "size": 1 })),
                track(json!({ "url": "data:image/png;base64,iVBORw0KGgo=" })),
                track(json!({ "url": "file:///home/citizen/cover.jpg" })),
                track(json!({ "url": "ftp://example.com/cover.jpg" })),
                track(json!({ "url": "javascript://example.com/%0Aalert(1)" })),
                track(json!({ "url": "https:example.com/cover.jpg" })),
                track(json!({ "url": "https://" })),
                track(json!({ "url": format!("{long_url}a") })),
            ]
            .iter()
            .map(Value::to_string)
            .chain([
                "".to_string(),
                "not json".to_string(),
                "{\"track\":".to_string(),
            ])
            .collect();

            for body in wrong {
                let response = post_display(
                    router.clone(),
                    &reserved.event_id,
                    Some(&authorization),
                    body.clone(),
                )
                .await;
                assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
                assert_eq!(error_code(response).await, "invalid_display", "{body}");
            }

            // No refused body changed the state.
            assert_eq!(
                read_display(router.clone(), &reserved.event_id).await,
                json!({ "track": null })
            );

            // The limit and the scheme accept these.
            for url in [long_url.as_str(), "HTTPS://example.com/cover.jpg"] {
                let state = track(json!({ "url": url }));
                publish_display_ok(router.clone(), &reserved, &state).await;
                assert_eq!(
                    read_display(router.clone(), &reserved.event_id).await,
                    state
                );
            }
        }

        #[tokio::test]
        async fn a_track_with_song_line_and_value_is_accepted() {
            let (router, _state, _clock, _dir, _) = display_app();
            let reserved = reserve_ok(router.clone(), None).await;

            let state_with_both = json!({
                "track": {
                    "artist": "Artist",
                    "title": "Title",
                    "artwork": null,
                    "songLine": "Artist - Title",
                    "value": { "eventGuid": "event123", "blockGuid": "block456" }
                }
            });
            publish_display_ok(router.clone(), &reserved, &state_with_both).await;
            let read_back = read_display(router.clone(), &reserved.event_id).await;
            assert_eq!(read_back, state_with_both);

            let state_with_only_song_line = json!({
                "track": {
                    "artist": "Artist",
                    "title": "Title",
                    "artwork": null,
                    "songLine": "Artist - Title"
                }
            });
            publish_display_ok(router.clone(), &reserved, &state_with_only_song_line).await;
            let read_back = read_display(router.clone(), &reserved.event_id).await;
            assert_eq!(read_back, state_with_only_song_line);

            let state_with_only_value = json!({
                "track": {
                    "artist": "Artist",
                    "title": "Title",
                    "artwork": null,
                    "value": { "eventGuid": "event789", "blockGuid": "block000" }
                }
            });
            publish_display_ok(router.clone(), &reserved, &state_with_only_value).await;
            let read_back = read_display(router.clone(), &reserved.event_id).await;
            assert_eq!(read_back, state_with_only_value);

            // The limit counts characters: 1,024 two-byte characters pass.
            let state_with_long_song_line = json!({
                "track": {
                    "artist": "Artist",
                    "title": "Title",
                    "artwork": null,
                    "songLine": "é".repeat(1_024)
                }
            });
            publish_display_ok(router.clone(), &reserved, &state_with_long_song_line).await;
            let read_back = read_display(router.clone(), &reserved.event_id).await;
            assert_eq!(read_back, state_with_long_song_line);
        }

        #[tokio::test]
        async fn a_subscriber_receives_states_with_song_line_and_value() {
            let (router, _state, _clock, _dir, _) = display_app();
            let reserved = reserve_ok(router.clone(), None).await;
            let mut body = open_display_stream(router.clone(), &reserved.event_id, None).await;

            let first_state = next_sse_chunk(&mut body).await;
            assert!(first_state.contains("event: display"), "{first_state}");

            let state = json!({
                "track": {
                    "artist": "Artist",
                    "title": "Title",
                    "artwork": null,
                    "songLine": "Artist - Title",
                    "value": { "eventGuid": "event123", "blockGuid": "block456" }
                }
            });
            publish_display_ok(router.clone(), &reserved, &state).await;
            let chunk = next_sse_chunk(&mut body).await;
            assert!(chunk.contains("event: display"), "{chunk}");
            assert_eq!(chunk_data(&chunk), state);
        }

        #[tokio::test]
        async fn invalid_song_line_and_value_give_400() {
            let (router, _state, _clock, _dir, _) = display_app();
            let reserved = reserve_ok(router.clone(), None).await;
            let authorization = format!("Bearer {}", reserved.broadcaster_token);

            let wrong: Vec<String> = [
                // Empty songLine
                json!({
                    "track": {
                        "artist": "Artist",
                        "title": "Title",
                        "artwork": null,
                        "songLine": ""
                    }
                }),
                // songLine too long (1025 characters)
                json!({
                    "track": {
                        "artist": "Artist",
                        "title": "Title",
                        "artwork": null,
                        "songLine": "a".repeat(1025)
                    }
                }),
                // songLine not a string
                json!({
                    "track": {
                        "artist": "Artist",
                        "title": "Title",
                        "artwork": null,
                        "songLine": 123
                    }
                }),
                // value with third key
                json!({
                    "track": {
                        "artist": "Artist",
                        "title": "Title",
                        "artwork": null,
                        "value": { "eventGuid": "event123", "blockGuid": "block456", "extra": "key" }
                    }
                }),
                // value with empty blockGuid
                json!({
                    "track": {
                        "artist": "Artist",
                        "title": "Title",
                        "artwork": null,
                        "value": { "eventGuid": "event123", "blockGuid": "" }
                    }
                }),
                // blockGuid of 129 characters
                json!({
                    "track": {
                        "artist": "Artist",
                        "title": "Title",
                        "artwork": null,
                        "value": { "eventGuid": "event123", "blockGuid": "b".repeat(129) }
                    }
                }),
                // value missing blockGuid
                json!({
                    "track": {
                        "artist": "Artist",
                        "title": "Title",
                        "artwork": null,
                        "value": { "eventGuid": "event123" }
                    }
                }),
                // value missing eventGuid
                json!({
                    "track": {
                        "artist": "Artist",
                        "title": "Title",
                        "artwork": null,
                        "value": { "blockGuid": "block456" }
                    }
                }),
                // Unknown track key
                json!({
                    "track": {
                        "artist": "Artist",
                        "title": "Title",
                        "artwork": null,
                        "unknown": "key"
                    }
                }),
            ]
            .iter()
            .map(Value::to_string)
            .collect();

            for body in wrong {
                let response = post_display(
                    router.clone(),
                    &reserved.event_id,
                    Some(&authorization),
                    body.clone(),
                )
                .await;
                assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
                assert_eq!(error_code(response).await, "invalid_display", "{body}");
            }

            // No refused body changed the state.
            assert_eq!(
                read_display(router.clone(), &reserved.event_id).await,
                json!({ "track": null })
            );
        }

        #[tokio::test]
        async fn a_display_publish_does_not_move_the_lease() {
            let (router, state, clock, _dir, _) = display_app();
            let reserved = reserve_ok(router.clone(), None).await;
            let response = publish(
                router.clone(),
                &reserved.event_id,
                Some(&reserved.broadcaster_token),
                json!({ "title": "on air" }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let metadata_uri = format!("/v1/liveitems/{}/metadata", reserved.event_id);
            let before: LatestMetadataResponse =
                read_json(get(router.clone(), metadata_uri.clone()).await).await;

            clock.store(105, Ordering::SeqCst);
            publish_display_ok(
                router.clone(),
                &reserved,
                &track(json!({ "url": "https://example.com/cover.jpg" })),
            )
            .await;

            let after: LatestMetadataResponse =
                read_json(get(router.clone(), metadata_uri).await).await;
            assert_eq!(after.renewed_at, before.renewed_at);
            assert_eq!(after.lease_expires_at, before.lease_expires_at);

            // The lease of 10 seconds runs from the publish at 100.
            clock.store(110, Ordering::SeqCst);
            assert_eq!(state.expire_leases().await, 1);
        }

        #[tokio::test]
        async fn a_lease_expiry_sets_the_state_to_null_and_sends_it() {
            let (router, state, clock, _dir, _) = display_app();
            let reserved = reserve_ok(router.clone(), None).await;
            let response = publish(
                router.clone(),
                &reserved.event_id,
                Some(&reserved.broadcaster_token),
                json!({ "title": "on air" }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            // Task 002: a display publish names only a held image.
            let jpeg = upload_ok(router.clone(), &reserved, &jpeg_bytes(1)).await;
            let playing = track(json!({ "sha256": jpeg, "mime": "image/jpeg" }));
            publish_display_ok(router.clone(), &reserved, &playing).await;
            let mut body = open_display_stream(router.clone(), &reserved.event_id, None).await;
            // The stream first sends the present state (ADR 0003, amended
            // 2026-10-04).
            let present = next_sse_chunk(&mut body).await;
            assert!(present.contains("event: display"), "{present}");

            clock.store(110, Ordering::SeqCst);
            assert_eq!(state.expire_leases().await, 1);

            let chunk = next_sse_chunk(&mut body).await;
            assert!(chunk.contains("event: display"), "{chunk}");
            assert!(chunk.contains("id: 2"), "{chunk}");
            assert_eq!(chunk_data(&chunk), json!({ "track": null }));
            assert_eq!(
                read_display(router.clone(), &reserved.event_id).await,
                json!({ "track": null })
            );

            // A reconnect receives the null state from the replay buffer.
            let mut replay =
                open_display_stream(router.clone(), &reserved.event_id, Some("1")).await;
            let chunk = next_sse_chunk(&mut replay).await;
            assert!(chunk.contains("id: 2"), "{chunk}");
            assert_eq!(chunk_data(&chunk), json!({ "track": null }));

            // A second expiry with a null state sends nothing more.
            clock.store(111, Ordering::SeqCst);
            let response = publish(
                router.clone(),
                &reserved.event_id,
                Some(&reserved.broadcaster_token),
                json!({ "title": "back" }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            clock.store(121, Ordering::SeqCst);
            assert_eq!(state.expire_leases().await, 1);
            assert_no_frame(&mut body).await;
        }

        #[tokio::test]
        async fn a_lease_end_with_no_snapshot_still_sets_the_display_to_null() {
            let (router, state, clock, _dir, _) = display_app();
            let reserved = reserve_ok(router.clone(), None).await;
            // A display state and no payload, as after a relay restart.
            let playing = track(json!({ "url": "https://example.com/cover.jpg" }));
            publish_display_ok(router.clone(), &reserved, &playing).await;
            let mut body = open_display_stream(router.clone(), &reserved.event_id, None).await;
            // The stream first sends the present state (ADR 0003, amended
            // 2026-10-04).
            let present = next_sse_chunk(&mut body).await;
            assert!(present.contains("event: display"), "{present}");

            clock.store(200, Ordering::SeqCst);
            // No snapshot was cleared, so the count stays 0.
            assert_eq!(state.expire_leases().await, 0);

            let chunk = next_sse_chunk(&mut body).await;
            assert!(chunk.contains("event: display"), "{chunk}");
            assert_eq!(chunk_data(&chunk), json!({ "track": null }));
            assert_eq!(
                read_display(router.clone(), &reserved.event_id).await,
                json!({ "track": null })
            );
        }

        #[tokio::test]
        async fn no_display_event_appears_on_the_live_value_transports() {
            let (router, state, clock, _dir, _) = display_app();
            let reserved = reserve_ok(router.clone(), None).await;
            let events_uri = format!("/v1/liveitems/{}/events", reserved.event_id);
            let response = open_stream(router.clone(), events_uri.clone(), None).await;
            assert_eq!(response.status(), StatusCode::OK);
            let mut events = response.into_body();

            let playing = track(json!({ "url": "https://example.com/cover.jpg" }));
            publish_display_ok(router.clone(), &reserved, &playing).await;
            assert_no_frame(&mut events).await;

            // The live value is unchanged by the display publish.
            let response = get(
                router.clone(),
                format!("/v1/liveitems/{}/remoteValue", reserved.event_id),
            )
            .await;
            assert_eq!(read_json::<Value>(response).await, json!({}));
            let response = get(
                router.clone(),
                format!("/v1/liveitems/{}/metadata", reserved.event_id),
            )
            .await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert_eq!(error_code(response).await, "metadata_not_found");

            // The next update on `/events` is the live value, with `seq` 1.
            let response = publish(
                router.clone(),
                &reserved.event_id,
                Some(&reserved.broadcaster_token),
                json!({ "title": "on air" }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let published: musicindex_live_relay::PublishMetadataResponse =
                read_json(response).await;
            assert_eq!(published.seq, 1);
            let chunk = next_sse_chunk(&mut events).await;
            assert!(chunk.contains("event: remoteValue"), "{chunk}");
            assert!(chunk.contains("id: 1"), "{chunk}");
            assert_eq!(chunk_data(&chunk), json!({ "title": "on air" }));

            // An expiry sends `{}` on `/events` and nothing of the display.
            clock.store(110, Ordering::SeqCst);
            assert_eq!(state.expire_leases().await, 1);
            let chunk = next_sse_chunk(&mut events).await;
            assert!(chunk.contains("event: remoteValue"), "{chunk}");
            assert_eq!(chunk_data(&chunk), json!({}));
            assert_no_frame(&mut events).await;

            // The replay buffer of `/events` holds no display state.
            let response = open_stream(router.clone(), events_uri, Some("0")).await;
            let mut replay = response.into_body();
            for expected in [json!({ "title": "on air" }), json!({})] {
                let chunk = next_sse_chunk(&mut replay).await;
                assert!(chunk.contains("event: remoteValue"), "{chunk}");
                assert!(!chunk.contains("display"), "{chunk}");
                assert_eq!(chunk_data(&chunk), expected);
            }
            assert_no_frame(&mut replay).await;
        }

        #[tokio::test]
        async fn the_display_publish_shares_the_publish_rate_limit() {
            let dir = tempfile::tempdir().expect("tempdir");
            let clock = Arc::new(AtomicU64::new(100));
            let state = RelayState::try_with_clock(
                AppConfig {
                    admin_token: Some(AdminToken::new(ADMIN_TOKEN)),
                    state_file: dir.path().join("reserved-items.sqlite3"),
                    max_publishes_per_event_per_sec: 2,
                    ..AppConfig::for_tests()
                },
                {
                    let clock = clock.clone();
                    Arc::new(move || clock.load(Ordering::SeqCst))
                },
            )
            .expect("open state");
            let router = app(state);
            let reserved = reserve_ok(router.clone(), None).await;
            let authorization = format!("Bearer {}", reserved.broadcaster_token);
            let body = json!({ "track": null }).to_string();

            let response = publish(
                router.clone(),
                &reserved.event_id,
                Some(&reserved.broadcaster_token),
                json!({ "title": "on air" }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            publish_display_ok(router.clone(), &reserved, &json!({ "track": null })).await;

            let response = post_display(
                router.clone(),
                &reserved.event_id,
                Some(&authorization),
                body.clone(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(error_code(response).await, "publish_rate_limited");

            clock.store(101, Ordering::SeqCst);
            let published =
                publish_display_ok(router.clone(), &reserved, &json!({ "track": null })).await;
            assert_eq!(published.seq, 2);
        }

        #[tokio::test]
        async fn the_display_stream_counts_against_the_sse_connection_limit() {
            let dir = tempfile::tempdir().expect("tempdir");
            let state = RelayState::try_new(AppConfig {
                admin_token: Some(AdminToken::new(ADMIN_TOKEN)),
                state_file: dir.path().join("reserved-items.sqlite3"),
                max_sse_connections: 1,
                ..AppConfig::for_tests()
            })
            .expect("open state");
            let router = app(state);
            let reserved = reserve_ok(router.clone(), None).await;

            let first = open_display_stream(router.clone(), &reserved.event_id, None).await;
            let uri = format!("/v1/liveitems/{}/display/events", reserved.event_id);
            let response = open_stream(router.clone(), uri.clone(), None).await;
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(error_code(response).await, "max_sse_connections_reached");

            drop(first);
            let response = open_stream(router, uri, None).await;
            assert_eq!(response.status(), StatusCode::OK);
        }

        #[tokio::test]
        async fn a_restart_gives_a_null_state() {
            let (router, _state, clock, _dir, state_file) = display_app();
            let reserved = reserve_ok(router.clone(), None).await;
            publish_display_ok(
                router,
                &reserved,
                &track(json!({ "url": "https://example.com/cover.jpg" })),
            )
            .await;

            let restarted = app(reserved_state_at(&state_file, &clock).expect("restart"));
            assert_eq!(
                read_display(restarted.clone(), &reserved.event_id).await,
                json!({ "track": null })
            );
            let published =
                publish_display_ok(restarted, &reserved, &json!({ "track": null })).await;
            assert_eq!(published.seq, 1);
        }

        #[tokio::test]
        async fn a_delete_stops_the_display_stream() {
            let (router, _state, _clock, _dir, _) = display_app();
            let reserved = reserve_ok(router.clone(), None).await;
            let mut body = open_display_stream(router.clone(), &reserved.event_id, None).await;
            // The stream first sends the present state (ADR 0003, amended
            // 2026-10-04).
            let present = next_sse_chunk(&mut body).await;
            assert!(present.contains("event: display"), "{present}");

            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method("DELETE")
                        .uri(format!("/v1/liveitems/reserved/{}", reserved.event_id))
                        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
                        .body(Body::empty())
                        .expect("delete request"),
                )
                .await
                .expect("delete response");
            assert_eq!(response.status(), StatusCode::NO_CONTENT);

            let frame = timeout(Duration::from_secs(2), body.frame())
                .await
                .expect("the stream ends");
            assert!(frame.is_none(), "the stream ended with a frame: {frame:?}");
            let response = get(
                router,
                format!("/v1/liveitems/{}/display", reserved.event_id),
            )
            .await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert_eq!(error_code(response).await, "event_not_found");
        }

        /// The reserved item routes hold the static segment `reserved`. An
        /// event identifier equal to `reserved` gets the same answers as
        /// before the display routes, on the display paths and on the
        /// paths of the other per-event routes.
        #[tokio::test]
        async fn the_identifier_reserved_keeps_its_previous_answers() {
            let (router, _state, _clock, _dir, _) = display_app();
            let cases = [
                (
                    "GET",
                    "/v1/liveitems/reserved/display",
                    StatusCode::METHOD_NOT_ALLOWED,
                ),
                (
                    "POST",
                    "/v1/liveitems/reserved/display",
                    StatusCode::METHOD_NOT_ALLOWED,
                ),
                (
                    "GET",
                    "/v1/liveitems/reserved/metadata",
                    StatusCode::METHOD_NOT_ALLOWED,
                ),
                (
                    "GET",
                    "/v1/liveitems/reserved/display/events",
                    StatusCode::NOT_FOUND,
                ),
            ];
            for (method, uri, status) in cases {
                let response = router
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method(method)
                            .uri(uri)
                            .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
                            .body(Body::from("{\"track\":null}"))
                            .expect("request"),
                    )
                    .await
                    .expect("response");
                assert_eq!(response.status(), status, "{method} {uri}");
            }

            // The delete route still reads `display` as an identifier.
            let response = router
                .oneshot(
                    Request::builder()
                        .method("DELETE")
                        .uri("/v1/liveitems/reserved/display")
                        .header(header::AUTHORIZATION, format!("Bearer {ADMIN_TOKEN}"))
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert_eq!(error_code(response).await, "event_not_found");
        }

        /// A small byte array that starts with the JPEG bytes.
        fn jpeg_bytes(seed: u8) -> Vec<u8> {
            let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0];
            bytes.extend([seed; 16]);
            bytes
        }

        /// A small byte array that starts with the PNG bytes.
        fn png_bytes(seed: u8) -> Vec<u8> {
            let mut bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
            bytes.extend([seed; 16]);
            bytes
        }

        fn sha256_of(bytes: &[u8]) -> String {
            use sha2::{Digest, Sha256};
            Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        }

        async fn put_artwork(
            router: axum::Router,
            event_id: &str,
            sha256: &str,
            authorization: Option<&str>,
            body: impl Into<Body>,
        ) -> axum::response::Response {
            let mut builder = Request::builder()
                .method("PUT")
                .uri(format!("/v1/liveitems/{event_id}/artwork/{sha256}"));
            if let Some(authorization) = authorization {
                builder = builder.header(header::AUTHORIZATION, authorization);
            }
            router
                .oneshot(builder.body(body.into()).expect("artwork request"))
                .await
                .expect("artwork response")
        }

        /// Uploads `bytes` with the token, asserts `200` and a new image, and
        /// returns the hash.
        async fn upload_ok(
            router: axum::Router,
            reserved: &ReserveEventResponse,
            bytes: &[u8],
        ) -> String {
            let sha256 = sha256_of(bytes);
            let response = put_artwork(
                router,
                &reserved.event_id,
                &sha256,
                Some(&format!("Bearer {}", reserved.broadcaster_token)),
                bytes.to_vec(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let uploaded: UploadArtworkResponse = read_json(response).await;
            assert_eq!(uploaded.event_id, reserved.event_id);
            assert_eq!(uploaded.sha256, sha256);
            assert!(uploaded.stored, "the image was held already");
            sha256
        }

        async fn get_artwork(
            router: axum::Router,
            event_id: &str,
            sha256: &str,
        ) -> axum::response::Response {
            get(router, format!("/v1/liveitems/{event_id}/artwork/{sha256}")).await
        }

        /// Asserts that the event holds the image with the hash `sha256`.
        async fn assert_held(router: axum::Router, event_id: &str, sha256: &str) {
            let response = get_artwork(router, event_id, sha256).await;
            assert_eq!(response.status(), StatusCode::OK, "{sha256} is not held");
        }

        /// Asserts that the event does not hold the image with the hash
        /// `sha256`.
        async fn assert_not_held(router: axum::Router, event_id: &str, sha256: &str) {
            let response = get_artwork(router, event_id, sha256).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{sha256} is held");
            assert_eq!(error_code(response).await, "artwork_not_found");
        }

        /// Tests for the artwork store of ADR 0003, task 002.
        mod artwork {
            use super::*;

            #[tokio::test]
            async fn an_upload_and_a_read_give_the_same_bytes_and_the_three_headers() {
                let (router, _state, _clock, _dir, _) = display_app();
                let reserved = reserve_ok(router.clone(), None).await;

                for (bytes, mime) in [(jpeg_bytes(1), "image/jpeg"), (png_bytes(1), "image/png")] {
                    let sha256 = sha256_of(&bytes);
                    let response = put_artwork(
                        router.clone(),
                        &reserved.event_id,
                        &sha256,
                        Some(&format!("Bearer {}", reserved.broadcaster_token)),
                        bytes.clone(),
                    )
                    .await;
                    assert_eq!(response.status(), StatusCode::OK);
                    let uploaded: UploadArtworkResponse = read_json(response).await;
                    assert_eq!(uploaded.mime, mime);
                    assert!(uploaded.stored);

                    let response = get_artwork(router.clone(), &reserved.event_id, &sha256).await;
                    assert_eq!(response.status(), StatusCode::OK);
                    let headers = response.headers();
                    assert_eq!(headers[header::CONTENT_TYPE], mime);
                    assert_eq!(
                        headers[header::CACHE_CONTROL],
                        "public, max-age=31536000, immutable"
                    );
                    assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
                    let body = to_bytes(response.into_body(), usize::MAX)
                        .await
                        .expect("read body");
                    assert_eq!(body.as_ref(), bytes.as_slice());
                }
            }

            #[tokio::test]
            async fn the_type_comes_from_the_bytes_only() {
                let (router, _state, _clock, _dir, _) = display_app();
                let reserved = reserve_ok(router.clone(), None).await;
                let bytes = jpeg_bytes(2);
                let sha256 = sha256_of(&bytes);

                let response = router
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method("PUT")
                            .uri(format!(
                                "/v1/liveitems/{}/artwork/{sha256}",
                                reserved.event_id
                            ))
                            .header(
                                header::AUTHORIZATION,
                                format!("Bearer {}", reserved.broadcaster_token),
                            )
                            .header(header::CONTENT_TYPE, "image/png")
                            .body(Body::from(bytes))
                            .expect("request"),
                    )
                    .await
                    .expect("response");
                assert_eq!(response.status(), StatusCode::OK);
                let uploaded: UploadArtworkResponse = read_json(response).await;
                assert_eq!(uploaded.mime, "image/jpeg");

                let response = get_artwork(router, &reserved.event_id, &sha256).await;
                assert_eq!(response.headers()[header::CONTENT_TYPE], "image/jpeg");
            }

            #[tokio::test]
            async fn a_wrong_hash_gives_400_sha256_mismatch() {
                let (router, _state, _clock, _dir, _) = display_app();
                let reserved = reserve_ok(router.clone(), None).await;
                let authorization = format!("Bearer {}", reserved.broadcaster_token);
                let bytes = jpeg_bytes(3);
                let right = sha256_of(&bytes);

                for wrong in [
                    sha256_of(&jpeg_bytes(4)),
                    right.to_uppercase(),
                    right[1..].to_string(),
                    format!("{right}0"),
                    SHA256.to_string(),
                ] {
                    let response = put_artwork(
                        router.clone(),
                        &reserved.event_id,
                        &wrong,
                        Some(&authorization),
                        bytes.clone(),
                    )
                    .await;
                    assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{wrong}");
                    assert_eq!(error_code(response).await, "sha256_mismatch", "{wrong}");
                    assert_not_held(router.clone(), &reserved.event_id, &wrong).await;
                }
                assert_not_held(router, &reserved.event_id, &right).await;
            }

            #[tokio::test]
            async fn bytes_that_are_not_jpeg_or_png_give_400_unsupported_image() {
                let (router, _state, _clock, _dir, _) = display_app();
                let reserved = reserve_ok(router.clone(), None).await;
                let authorization = format!("Bearer {}", reserved.broadcaster_token);

                let wrong: Vec<Vec<u8>> = vec![
                    Vec::new(),
                    b"GIF89a\x01\x00\x01\x00".to_vec(),
                    b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec(),
                    vec![0xFF, 0xD8],
                    vec![0xFF, 0xD9, 0xFF, 0xE0],
                    vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A],
                    vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0B, 0x00],
                    [b"x".as_slice(), &jpeg_bytes(5)].concat(),
                ];
                for bytes in wrong {
                    let sha256 = sha256_of(&bytes);
                    let response = put_artwork(
                        router.clone(),
                        &reserved.event_id,
                        &sha256,
                        Some(&authorization),
                        bytes.clone(),
                    )
                    .await;
                    assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{bytes:?}");
                    assert_eq!(error_code(response).await, "unsupported_image", "{bytes:?}");
                    assert_not_held(router.clone(), &reserved.event_id, &sha256).await;
                }
            }

            #[tokio::test]
            async fn a_body_over_the_limit_gives_413() {
                let dir = tempfile::tempdir().expect("tempdir");
                let state = RelayState::try_new(AppConfig {
                    admin_token: Some(AdminToken::new(ADMIN_TOKEN)),
                    state_file: dir.path().join("reserved-items.sqlite3"),
                    artwork_max_bytes: 1_024,
                    ..AppConfig::for_tests()
                })
                .expect("open state");
                let router = app(state);
                let reserved = reserve_ok(router.clone(), None).await;
                let authorization = format!("Bearer {}", reserved.broadcaster_token);
                let sized = |len: usize| {
                    let mut bytes = jpeg_bytes(6);
                    bytes.resize(len, 0);
                    bytes
                };

                // A body of exactly the limit is accepted.
                upload_ok(router.clone(), &reserved, &sized(1_024)).await;

                // A streamed body over the limit gets the JSON error.
                let over = sized(1_025);
                let sha256 = sha256_of(&over);
                let response = put_artwork(
                    router.clone(),
                    &reserved.event_id,
                    &sha256,
                    Some(&authorization),
                    over.clone(),
                )
                .await;
                assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
                assert_eq!(error_code(response).await, "payload_too_large");

                // The credential comes before the body.
                let response = put_artwork(
                    router.clone(),
                    &reserved.event_id,
                    &sha256,
                    None,
                    over.clone(),
                )
                .await;
                assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
                assert_eq!(error_code(response).await, "missing_bearer_token");

                // A request that states its length is refused before the
                // handler runs.
                let response = router
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method("PUT")
                            .uri(format!(
                                "/v1/liveitems/{}/artwork/{sha256}",
                                reserved.event_id
                            ))
                            .header(header::AUTHORIZATION, &authorization)
                            .header(header::CONTENT_LENGTH, over.len())
                            .body(Body::from(over))
                            .expect("request"),
                    )
                    .await
                    .expect("response");
                assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

                assert_not_held(router, &reserved.event_id, &sha256).await;
            }

            #[tokio::test]
            async fn the_default_limit_accepts_524288_bytes_and_refuses_one_more() {
                let (router, _state, _clock, _dir, _) = display_app();
                let reserved = reserve_ok(router.clone(), None).await;
                let mut bytes = jpeg_bytes(7);
                bytes.resize(524_288, 0);
                upload_ok(router.clone(), &reserved, &bytes).await;

                bytes.push(0);
                let response = put_artwork(
                    router,
                    &reserved.event_id,
                    &sha256_of(&bytes),
                    Some(&format!("Bearer {}", reserved.broadcaster_token)),
                    bytes,
                )
                .await;
                assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
                assert_eq!(error_code(response).await, "payload_too_large");
            }

            #[tokio::test]
            async fn a_second_upload_of_one_image_gives_200_with_no_change() {
                let (router, _state, _clock, _dir, _) = display_app();
                let reserved = reserve_ok(router.clone(), None).await;
                let first = upload_ok(router.clone(), &reserved, &jpeg_bytes(1)).await;
                let second = upload_ok(router.clone(), &reserved, &jpeg_bytes(2)).await;

                let response = put_artwork(
                    router.clone(),
                    &reserved.event_id,
                    &first,
                    Some(&format!("Bearer {}", reserved.broadcaster_token)),
                    jpeg_bytes(1),
                )
                .await;
                assert_eq!(response.status(), StatusCode::OK);
                let uploaded: UploadArtworkResponse = read_json(response).await;
                assert!(!uploaded.stored);
                assert_eq!(uploaded.mime, "image/jpeg");

                // The order did not change: a third image removes the first.
                let third = upload_ok(router.clone(), &reserved, &jpeg_bytes(3)).await;
                assert_not_held(router.clone(), &reserved.event_id, &first).await;
                assert_held(router.clone(), &reserved.event_id, &second).await;
                assert_held(router, &reserved.event_id, &third).await;
            }

            #[tokio::test]
            async fn a_display_publish_with_an_image_that_is_not_held_gives_409_artwork_missing() {
                let (router, _state, _clock, _dir, _) = display_app();
                let reserved = reserve_ok(router.clone(), None).await;
                let authorization = format!("Bearer {}", reserved.broadcaster_token);
                let bytes = jpeg_bytes(1);
                let sha256 = sha256_of(&bytes);
                let playing = track(json!({ "sha256": sha256, "mime": "image/jpeg" }));

                let response = post_display(
                    router.clone(),
                    &reserved.event_id,
                    Some(&authorization),
                    playing.to_string(),
                )
                .await;
                assert_eq!(response.status(), StatusCode::CONFLICT);
                assert_eq!(error_code(response).await, "artwork_missing");
                assert_eq!(
                    read_display(router.clone(), &reserved.event_id).await,
                    json!({ "track": null })
                );

                // After the upload, the same publish is accepted.
                upload_ok(router.clone(), &reserved, &bytes).await;
                let published = publish_display_ok(router.clone(), &reserved, &playing).await;
                assert_eq!(published.seq, 1);
            }

            #[tokio::test]
            async fn a_mime_that_is_not_the_stored_type_gives_400_invalid_display() {
                let (router, _state, _clock, _dir, _) = display_app();
                let reserved = reserve_ok(router.clone(), None).await;
                let authorization = format!("Bearer {}", reserved.broadcaster_token);
                let jpeg = upload_ok(router.clone(), &reserved, &jpeg_bytes(1)).await;
                let png = upload_ok(router.clone(), &reserved, &png_bytes(1)).await;

                for (sha256, mime) in [(&jpeg, "image/png"), (&png, "image/jpeg")] {
                    let response = post_display(
                        router.clone(),
                        &reserved.event_id,
                        Some(&authorization),
                        track(json!({ "sha256": sha256, "mime": mime })).to_string(),
                    )
                    .await;
                    assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{mime}");
                    assert_eq!(error_code(response).await, "invalid_display", "{mime}");
                }
                assert_eq!(
                    read_display(router.clone(), &reserved.event_id).await,
                    json!({ "track": null })
                );
                // A refused publish removed no image.
                assert_held(router.clone(), &reserved.event_id, &jpeg).await;
                assert_held(router, &reserved.event_id, &png).await;
            }

            #[tokio::test]
            async fn after_three_display_states_with_three_images_the_first_image_gives_404() {
                let (router, _state, _clock, _dir, _) = display_app();
                let reserved = reserve_ok(router.clone(), None).await;

                let mut hashes = Vec::new();
                for seed in 1..=3 {
                    let sha256 = upload_ok(router.clone(), &reserved, &jpeg_bytes(seed)).await;
                    let state = track(json!({ "sha256": sha256, "mime": "image/jpeg" }));
                    publish_display_ok(router.clone(), &reserved, &state).await;
                    hashes.push(sha256);
                }
                assert_not_held(router.clone(), &reserved.event_id, &hashes[0]).await;
                assert_held(router.clone(), &reserved.event_id, &hashes[1]).await;
                assert_held(router, &reserved.event_id, &hashes[2]).await;
            }

            #[tokio::test]
            async fn a_display_publish_removes_each_image_of_neither_the_present_nor_the_previous_state()
             {
                let (router, _state, _clock, _dir, _) = display_app();
                let reserved = reserve_ok(router.clone(), None).await;
                let named = upload_ok(router.clone(), &reserved, &jpeg_bytes(1)).await;
                let unnamed = upload_ok(router.clone(), &reserved, &jpeg_bytes(2)).await;

                // A publish removes an upload that no state names.
                let playing = track(json!({ "sha256": named, "mime": "image/jpeg" }));
                publish_display_ok(router.clone(), &reserved, &playing).await;
                assert_held(router.clone(), &reserved.event_id, &named).await;
                assert_not_held(router.clone(), &reserved.event_id, &unnamed).await;

                // A state with no relay image keeps the image of the most
                // recent earlier state with one (ADR 0003, amended 2026-10-05).
                let url = track(json!({ "url": "https://example.com/cover.jpg" }));
                publish_display_ok(router.clone(), &reserved, &url).await;
                assert_held(router.clone(), &reserved.event_id, &named).await;

                publish_display_ok(router.clone(), &reserved, &json!({ "track": null })).await;
                assert_held(router.clone(), &reserved.event_id, &named).await;

                // Two newer images replace it.
                for seed in 3..=4 {
                    let sha256 = upload_ok(router.clone(), &reserved, &jpeg_bytes(seed)).await;
                    let state = track(json!({ "sha256": sha256, "mime": "image/jpeg" }));
                    publish_display_ok(router.clone(), &reserved, &state).await;
                }
                assert_not_held(router, &reserved.event_id, &named).await;
            }

            #[tokio::test]
            async fn two_null_states_keep_the_image_of_the_last_track() {
                let (router, _state, _clock, _dir, _) = display_app();
                let reserved = reserve_ok(router.clone(), None).await;
                let sha256 = upload_ok(router.clone(), &reserved, &jpeg_bytes(1)).await;
                let playing = track(json!({ "sha256": sha256, "mime": "image/jpeg" }));
                publish_display_ok(router.clone(), &reserved, &playing).await;

                // A client behind the stream still shows this track.
                publish_display_ok(router.clone(), &reserved, &json!({ "track": null })).await;
                publish_display_ok(router.clone(), &reserved, &json!({ "track": null })).await;
                assert_held(router, &reserved.event_id, &sha256).await;
            }

            #[tokio::test]
            async fn three_uploads_with_no_publish_leave_at_most_two_images() {
                let (router, _state, _clock, _dir, _) = display_app();
                let reserved = reserve_ok(router.clone(), None).await;

                let hashes = [
                    upload_ok(router.clone(), &reserved, &jpeg_bytes(1)).await,
                    upload_ok(router.clone(), &reserved, &jpeg_bytes(2)).await,
                    upload_ok(router.clone(), &reserved, &jpeg_bytes(3)).await,
                ];
                assert_not_held(router.clone(), &reserved.event_id, &hashes[0]).await;
                assert_held(router.clone(), &reserved.event_id, &hashes[1]).await;
                assert_held(router, &reserved.event_id, &hashes[2]).await;
            }

            #[tokio::test]
            async fn an_upload_never_removes_the_image_of_the_present_state() {
                let (router, _state, _clock, _dir, _) = display_app();
                let reserved = reserve_ok(router.clone(), None).await;

                let previous = upload_ok(router.clone(), &reserved, &jpeg_bytes(1)).await;
                let state = track(json!({ "sha256": previous, "mime": "image/jpeg" }));
                publish_display_ok(router.clone(), &reserved, &state).await;
                let present = upload_ok(router.clone(), &reserved, &jpeg_bytes(2)).await;
                let state = track(json!({ "sha256": present, "mime": "image/jpeg" }));
                publish_display_ok(router.clone(), &reserved, &state).await;
                assert_held(router.clone(), &reserved.event_id, &previous).await;

                // The first upload removes the image of the previous state.
                let first = upload_ok(router.clone(), &reserved, &jpeg_bytes(3)).await;
                assert_not_held(router.clone(), &reserved.event_id, &previous).await;
                assert_held(router.clone(), &reserved.event_id, &present).await;

                // Each later upload removes the earlier upload.
                let mut last = first;
                for seed in 4..=6 {
                    let next = upload_ok(router.clone(), &reserved, &jpeg_bytes(seed)).await;
                    assert_not_held(router.clone(), &reserved.event_id, &last).await;
                    assert_held(router.clone(), &reserved.event_id, &next).await;
                    assert_held(router.clone(), &reserved.event_id, &present).await;
                    last = next;
                }
                assert_eq!(
                    read_display(router, &reserved.event_id).await,
                    track(json!({ "sha256": present, "mime": "image/jpeg" }))
                );
            }

            #[tokio::test]
            async fn a_lease_expiry_with_a_snapshot_removes_the_images() {
                let (router, state, clock, _dir, _) = display_app();
                let reserved = reserve_ok(router.clone(), None).await;
                let response = publish(
                    router.clone(),
                    &reserved.event_id,
                    Some(&reserved.broadcaster_token),
                    json!({ "title": "on air" }),
                )
                .await;
                assert_eq!(response.status(), StatusCode::OK);
                let named = upload_ok(router.clone(), &reserved, &jpeg_bytes(1)).await;
                let state_with_image = track(json!({ "sha256": named, "mime": "image/jpeg" }));
                publish_display_ok(router.clone(), &reserved, &state_with_image).await;
                let unnamed = upload_ok(router.clone(), &reserved, &png_bytes(1)).await;

                // Before the lease ends, the images stay.
                clock.store(109, Ordering::SeqCst);
                assert_eq!(state.expire_leases().await, 0);
                assert_held(router.clone(), &reserved.event_id, &named).await;
                assert_held(router.clone(), &reserved.event_id, &unnamed).await;

                clock.store(110, Ordering::SeqCst);
                assert_eq!(state.expire_leases().await, 1);
                assert_not_held(router.clone(), &reserved.event_id, &named).await;
                assert_not_held(router.clone(), &reserved.event_id, &unnamed).await;
                assert_eq!(
                    read_display(router.clone(), &reserved.event_id).await,
                    json!({ "track": null })
                );

                // A display state that names the image needs a new upload.
                let response = post_display(
                    router,
                    &reserved.event_id,
                    Some(&format!("Bearer {}", reserved.broadcaster_token)),
                    state_with_image.to_string(),
                )
                .await;
                assert_eq!(response.status(), StatusCode::CONFLICT);
                assert_eq!(error_code(response).await, "artwork_missing");
            }

            #[tokio::test]
            async fn a_lease_end_with_no_snapshot_removes_the_images() {
                let (router, state, clock, _dir, _) = display_app();
                let reserved = reserve_ok(router.clone(), None).await;
                // Images and no payload, as after a relay restart. One image
                // is named by a display state, and one is not.
                let named = upload_ok(router.clone(), &reserved, &jpeg_bytes(1)).await;
                let state_with_image = track(json!({ "sha256": named, "mime": "image/jpeg" }));
                publish_display_ok(router.clone(), &reserved, &state_with_image).await;
                let unnamed = upload_ok(router.clone(), &reserved, &png_bytes(1)).await;

                clock.store(200, Ordering::SeqCst);
                // No snapshot was cleared, so the count stays 0.
                assert_eq!(state.expire_leases().await, 0);
                assert_not_held(router.clone(), &reserved.event_id, &named).await;
                assert_not_held(router.clone(), &reserved.event_id, &unnamed).await;

                // With a null display state, an upload after the lease end
                // also goes at the next expiry pass.
                let late = upload_ok(router.clone(), &reserved, &jpeg_bytes(2)).await;
                assert_eq!(state.expire_leases().await, 0);
                assert_not_held(router, &reserved.event_id, &late).await;
            }

            #[tokio::test]
            async fn an_ephemeral_event_gives_409_event_not_reserved_on_both_routes() {
                let (router, _state, _clock, _dir, _) = display_app();
                let ephemeral = create_event(router.clone()).await;
                let bytes = jpeg_bytes(1);
                let sha256 = sha256_of(&bytes);

                let response = put_artwork(
                    router.clone(),
                    &ephemeral.event_id,
                    &sha256,
                    Some(&format!("Bearer {}", ephemeral.broadcaster_token)),
                    bytes,
                )
                .await;
                assert_eq!(response.status(), StatusCode::CONFLICT);
                assert_eq!(error_code(response).await, "event_not_reserved");

                let response = get_artwork(router, &ephemeral.event_id, &sha256).await;
                assert_eq!(response.status(), StatusCode::CONFLICT);
                assert_eq!(error_code(response).await, "event_not_reserved");
            }

            #[tokio::test]
            async fn a_bad_credential_gives_401_or_403_and_an_unknown_event_gives_404() {
                let (router, _state, _clock, _dir, _) = display_app();
                let reserved = reserve_ok(router.clone(), Some("first")).await;
                let other = reserve_ok(router.clone(), Some("second")).await;
                let bytes = jpeg_bytes(1);
                let sha256 = sha256_of(&bytes);

                let cases = [
                    (None, StatusCode::UNAUTHORIZED, "missing_bearer_token"),
                    (
                        Some("Token abc".to_string()),
                        StatusCode::UNAUTHORIZED,
                        "invalid_bearer_token",
                    ),
                    (
                        Some("Bearer wrong".to_string()),
                        StatusCode::FORBIDDEN,
                        "invalid_token",
                    ),
                    (
                        Some(format!("Bearer {}", other.broadcaster_token)),
                        StatusCode::FORBIDDEN,
                        "invalid_token",
                    ),
                    (
                        Some(format!("Bearer {ADMIN_TOKEN}")),
                        StatusCode::FORBIDDEN,
                        "invalid_token",
                    ),
                ];
                for (authorization, status, code) in cases {
                    let response = put_artwork(
                        router.clone(),
                        &reserved.event_id,
                        &sha256,
                        authorization.as_deref(),
                        bytes.clone(),
                    )
                    .await;
                    assert_eq!(response.status(), status, "{authorization:?}");
                    assert_eq!(error_code(response).await, code, "{authorization:?}");
                }
                assert_not_held(router.clone(), &reserved.event_id, &sha256).await;

                let response = put_artwork(
                    router.clone(),
                    "missing",
                    &sha256,
                    Some(&format!("Bearer {}", reserved.broadcaster_token)),
                    bytes,
                )
                .await;
                assert_eq!(response.status(), StatusCode::NOT_FOUND);
                assert_eq!(error_code(response).await, "event_not_found");
                let response = get_artwork(router, "missing", &sha256).await;
                assert_eq!(response.status(), StatusCode::NOT_FOUND);
                assert_eq!(error_code(response).await, "event_not_found");
            }

            #[tokio::test]
            async fn the_upload_shares_the_publish_rate_limit() {
                let dir = tempfile::tempdir().expect("tempdir");
                let clock = Arc::new(AtomicU64::new(100));
                let state = RelayState::try_with_clock(
                    AppConfig {
                        admin_token: Some(AdminToken::new(ADMIN_TOKEN)),
                        state_file: dir.path().join("reserved-items.sqlite3"),
                        max_publishes_per_event_per_sec: 2,
                        ..AppConfig::for_tests()
                    },
                    {
                        let clock = clock.clone();
                        Arc::new(move || clock.load(Ordering::SeqCst))
                    },
                )
                .expect("open state");
                let router = app(state);
                let reserved = reserve_ok(router.clone(), None).await;

                let response = publish(
                    router.clone(),
                    &reserved.event_id,
                    Some(&reserved.broadcaster_token),
                    json!({ "title": "on air" }),
                )
                .await;
                assert_eq!(response.status(), StatusCode::OK);
                upload_ok(router.clone(), &reserved, &jpeg_bytes(1)).await;

                let bytes = jpeg_bytes(2);
                let sha256 = sha256_of(&bytes);
                let response = put_artwork(
                    router.clone(),
                    &reserved.event_id,
                    &sha256,
                    Some(&format!("Bearer {}", reserved.broadcaster_token)),
                    bytes.clone(),
                )
                .await;
                assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
                assert_eq!(error_code(response).await, "publish_rate_limited");
                assert_not_held(router.clone(), &reserved.event_id, &sha256).await;

                clock.store(101, Ordering::SeqCst);
                upload_ok(router, &reserved, &bytes).await;
            }

            #[tokio::test]
            async fn a_restart_holds_no_image() {
                let (router, _state, clock, _dir, state_file) = display_app();
                let reserved = reserve_ok(router.clone(), None).await;
                let sha256 = upload_ok(router, &reserved, &jpeg_bytes(1)).await;

                let restarted = app(reserved_state_at(&state_file, &clock).expect("restart"));
                assert_not_held(restarted, &reserved.event_id, &sha256).await;
            }
        }
    }
}
