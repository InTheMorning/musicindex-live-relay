use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    net::SocketAddr,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

use axum::{
    Json, Router,
    extract::{Path, State as AxumState},
    http::{HeaderMap, StatusCode, header},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use socketioxide::{
    SocketIo,
    extract::{SocketRef, State as SocketState},
};
use subtle::ConstantTimeEq;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::sync::{RwLock, broadcast};
use tower_http::{cors::CorsLayer, limit::RequestBodyLimitLayer};

const DEFAULT_BIND: &str = "127.0.0.1:8018";
const DEFAULT_MAX_ACTIVE_EVENTS: usize = 10_000;
const DEFAULT_MAX_SSE_CONNECTIONS: usize = 1_000;
const DEFAULT_TTL_SECS: u64 = 24 * 60 * 60;
const DEFAULT_MAX_PUBLISHES_PER_EVENT_PER_SEC: u32 = 20;
const DEFAULT_MAX_CREATES_PER_SEC: u32 = 50;
const DEFAULT_LEASE_SECS: u64 = 90;
const MIN_LEASE_SECS: u64 = 10;
const METADATA_BODY_LIMIT_BYTES: usize = 64 * 1024;
const REPLAY_CAPACITY: usize = 100;
const BROADCAST_CAPACITY: usize = 128;
const TOKEN_BYTES: usize = 32;
const EVENT_ID_BYTES: usize = 16;

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub bind: SocketAddr,
    pub max_active_events: usize,
    pub max_sse_connections: usize,
    pub ttl: Duration,
    pub max_publishes_per_event_per_sec: u32,
    pub max_creates_per_sec: u32,
    pub lease: Duration,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            bind: env_parse("BIND", DEFAULT_BIND.parse().expect("default bind is valid"))?,
            max_active_events: env_parse("MAX_ACTIVE_EVENTS", DEFAULT_MAX_ACTIVE_EVENTS)?,
            max_sse_connections: env_parse("MAX_SSE_CONNECTIONS", DEFAULT_MAX_SSE_CONNECTIONS)?,
            ttl: Duration::from_secs(env_parse("EVENT_TTL_SECS", DEFAULT_TTL_SECS)?),
            max_publishes_per_event_per_sec: env_parse(
                "MAX_PUBLISHES_PER_EVENT_PER_SEC",
                DEFAULT_MAX_PUBLISHES_PER_EVENT_PER_SEC,
            )?,
            max_creates_per_sec: env_parse("MAX_CREATES_PER_SEC", DEFAULT_MAX_CREATES_PER_SEC)?,
            lease: Duration::from_secs(parse_lease_secs()?),
        })
    }

    pub fn for_tests() -> Self {
        Self {
            bind: DEFAULT_BIND.parse().expect("default bind is valid"),
            max_active_events: DEFAULT_MAX_ACTIVE_EVENTS,
            max_sse_connections: DEFAULT_MAX_SSE_CONNECTIONS,
            ttl: Duration::from_secs(DEFAULT_TTL_SECS),
            max_publishes_per_event_per_sec: DEFAULT_MAX_PUBLISHES_PER_EVENT_PER_SEC,
            max_creates_per_sec: DEFAULT_MAX_CREATES_PER_SEC,
            lease: Duration::from_secs(DEFAULT_LEASE_SECS),
        }
    }
}

fn parse_lease_secs() -> Result<u64, ConfigError> {
    let secs = env_parse("LEASE_SECS", DEFAULT_LEASE_SECS)?;
    if secs < MIN_LEASE_SECS {
        return Err(ConfigError {
            key: "LEASE_SECS",
            value: secs.to_string(),
            message: format!("must be {MIN_LEASE_SECS} or more"),
        });
    }
    Ok(secs)
}

#[derive(Debug)]
pub struct ConfigError {
    key: &'static str,
    value: String,
    message: String,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid {} value {:?}: {}",
            self.key, self.value, self.message
        )
    }
}

impl std::error::Error for ConfigError {}

fn env_parse<T>(key: &'static str, default: T) -> Result<T, ConfigError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match std::env::var(key) {
        Ok(value) => value.parse().map_err(|err: T::Err| ConfigError {
            key,
            value,
            message: err.to_string(),
        }),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(std::env::VarError::NotUnicode(value)) => Err(ConfigError {
            key,
            value: format!("{value:?}"),
            message: "not valid unicode".to_string(),
        }),
    }
}

#[derive(Clone)]
pub struct RelayState {
    inner: Arc<RelayStateInner>,
}

struct RelayStateInner {
    config: AppConfig,
    events: RwLock<HashMap<String, Arc<LiveEventState>>>,
    sse_connections: AtomicUsize,
    create_window: AtomicU64,
    socket_io: OnceLock<SocketIo>,
    clock: Clock,
}

type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

impl RelayState {
    pub fn new(config: AppConfig) -> Self {
        Self::with_clock(config, Arc::new(epoch_seconds))
    }

    pub fn with_clock(config: AppConfig, clock: Clock) -> Self {
        Self {
            inner: Arc::new(RelayStateInner {
                config,
                events: RwLock::new(HashMap::new()),
                sse_connections: AtomicUsize::new(0),
                create_window: AtomicU64::new(0),
                socket_io: OnceLock::new(),
                clock,
            }),
        }
    }

    fn now(&self) -> u64 {
        (self.inner.clock)()
    }

    pub async fn create_event(&self) -> Result<CreateEventResponse, ApiError> {
        if !try_acquire_window_slot(
            &self.inner.create_window,
            self.now(),
            self.inner.config.max_creates_per_sec,
        ) {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "create_rate_limited",
            ));
        }

        let mut events = self.inner.events.write().await;
        if events.len() >= self.inner.config.max_active_events {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "max_active_events_reached",
            ));
        }

        let event_id = unique_event_id(&events);
        let token = random_urlsafe(TOKEN_BYTES);
        let event = Arc::new(LiveEventState::new(hash_token(&token), self.now()));

        events.insert(event_id.clone(), event);

        Ok(CreateEventResponse {
            event_id: event_id.clone(),
            broadcaster_token: token,
            metadata_url: format!("/v1/liveitems/{event_id}/metadata"),
            remote_value_url: format!("/v1/liveitems/{event_id}/remoteValue"),
            events_url: format!("/v1/liveitems/{event_id}/events"),
            socket_io_url: format!("/event?event_id={event_id}"),
        })
    }

    async fn get_event(&self, event_id: &str) -> Result<Arc<LiveEventState>, ApiError> {
        self.inner
            .events
            .read()
            .await
            .get(event_id)
            .cloned()
            .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "event_not_found"))
    }

    pub async fn publish_metadata(
        &self,
        event_id: &str,
        token: &str,
        body: PublishMetadataRequest,
    ) -> Result<PublishMetadataResponse, ApiError> {
        let event = self.get_event(event_id).await?;
        if !event.token_matches(token) {
            return Err(ApiError::new(StatusCode::FORBIDDEN, "invalid_token"));
        }
        let metadata = body.into_metadata(event_id)?;

        let now = self.now();
        if !event.try_acquire_publish_slot(now, self.inner.config.max_publishes_per_event_per_sec) {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "publish_rate_limited",
            ));
        }
        event.touch(now);
        event.renew_lease(now);

        let mut inner = event.inner.write().await;
        inner.seq += 1;
        let seq = inner.seq;

        let snapshot = MetadataSnapshot {
            event_id: event_id.to_string(),
            seq,
            updated_at: format_timestamp(now),
            metadata,
        };

        inner.latest = Some(snapshot.clone());
        inner.replay.push_back(snapshot.clone());
        while inner.replay.len() > REPLAY_CAPACITY {
            inner.replay.pop_front();
        }

        let metadata = snapshot.metadata.clone();
        drop(inner);
        let _ = event.sender.send(snapshot);
        self.emit_socket_remote_value(event_id, &metadata).await;

        Ok(PublishMetadataResponse {
            event_id: event_id.to_string(),
            accepted: true,
            seq,
            lease_secs: self.inner.config.lease.as_secs(),
            keepalive_interval_secs: self.keepalive_interval_secs(),
        })
    }

    fn keepalive_interval_secs(&self) -> u64 {
        self.inner.config.lease.as_secs() / 3
    }

    pub async fn latest_metadata(
        &self,
        event_id: &str,
    ) -> Result<LatestMetadataResponse, ApiError> {
        let event = self.get_event(event_id).await?;
        let inner = event.inner.read().await;
        let snapshot = inner
            .latest
            .clone()
            .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "metadata_not_found"))?;
        drop(inner);

        let renewed_at = event.renewed_at.load(Ordering::Relaxed);
        let lease_expires_at = renewed_at + self.inner.config.lease.as_secs();

        Ok(LatestMetadataResponse {
            event_id: snapshot.event_id,
            seq: snapshot.seq,
            updated_at: snapshot.updated_at,
            renewed_at: format_timestamp(renewed_at),
            lease_expires_at: format_timestamp(lease_expires_at),
            metadata: snapshot.metadata,
        })
    }

    /// Renews the lease of an event that a broadcaster keeps on air.
    ///
    /// The order of checks is: the event exists, the token matches, the
    /// publish rate limit allows the request, and the event holds a
    /// snapshot. Only a request that clears every check renews the lease.
    ///
    /// # Errors
    ///
    /// Returns `404 event_not_found`, `403 invalid_token`,
    /// `429 publish_rate_limited`, or `409 lease_expired`.
    pub async fn keepalive(
        &self,
        event_id: &str,
        token: &str,
    ) -> Result<KeepaliveResponse, ApiError> {
        let event = self.get_event(event_id).await?;
        if !event.token_matches(token) {
            return Err(ApiError::new(StatusCode::FORBIDDEN, "invalid_token"));
        }

        let now = self.now();
        if !event.try_acquire_publish_slot(now, self.inner.config.max_publishes_per_event_per_sec) {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "publish_rate_limited",
            ));
        }

        // Renew while the read lock is held. `expire_leases` needs the write
        // lock to remove the snapshot, so it cannot remove it between this
        // check and the renewal.
        let inner = event.inner.read().await;
        if inner.latest.is_none() {
            return Err(ApiError::new(StatusCode::CONFLICT, "lease_expired"));
        }
        event.renew_lease(now);
        drop(inner);
        event.touch(now);

        Ok(KeepaliveResponse {
            event_id: event_id.to_string(),
            lease_expires_at: format_timestamp(now + self.inner.config.lease.as_secs()),
            keepalive_interval_secs: self.keepalive_interval_secs(),
        })
    }

    async fn subscribe(
        &self,
        event_id: &str,
        last_event_id: Option<u64>,
    ) -> Result<EventSubscription, ApiError> {
        let previous = self.inner.sse_connections.fetch_add(1, Ordering::AcqRel);
        if previous >= self.inner.config.max_sse_connections {
            self.inner.sse_connections.fetch_sub(1, Ordering::AcqRel);
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "max_sse_connections_reached",
            ));
        }

        let event = match self.get_event(event_id).await {
            Ok(event) => event,
            Err(err) => {
                self.inner.sse_connections.fetch_sub(1, Ordering::AcqRel);
                return Err(err);
            }
        };

        event.touch(self.now());
        let replay = {
            let inner = event.inner.read().await;
            match last_event_id {
                Some(last_event_id) => inner
                    .replay
                    .iter()
                    .filter(|snapshot| snapshot.seq > last_event_id)
                    .cloned()
                    .collect(),
                None => Vec::new(),
            }
        };
        let receiver = event.sender.subscribe();

        Ok(EventSubscription {
            state: self.clone(),
            event,
            replay,
            receiver,
        })
    }

    pub async fn cleanup_expired(&self) -> usize {
        let cutoff = self.now().saturating_sub(self.inner.config.ttl.as_secs());
        let mut events = self.inner.events.write().await;
        let before = events.len();
        events.retain(|_, event| event.last_activity.load(Ordering::Relaxed) >= cutoff);
        before - events.len()
    }

    /// Clears the snapshot of each event whose lease has expired.
    ///
    /// An event's lease expires when it holds a snapshot and
    /// `now - renewed_at` is at least `LEASE_SECS`. Clearing a lease removes
    /// the snapshot, advances `seq`, adds a `{}` update to the replay buffer,
    /// and emits `{}` to the SSE and Socket.IO subscribers. An event with no
    /// snapshot is left as is. Expiry never removes the event itself and
    /// never changes its idle-TTL activity time.
    ///
    /// Returns the count of events cleared this way.
    pub async fn expire_leases(&self) -> usize {
        let now = self.now();
        let lease_secs = self.inner.config.lease.as_secs();

        let candidates: Vec<(String, Arc<LiveEventState>)> = self
            .inner
            .events
            .read()
            .await
            .iter()
            .map(|(event_id, event)| (event_id.clone(), Arc::clone(event)))
            .collect();

        let mut expired_count = 0;
        for (event_id, event) in candidates {
            let renewed_at = event.renewed_at.load(Ordering::Relaxed);
            if now.saturating_sub(renewed_at) < lease_secs {
                continue;
            }

            let mut inner = event.inner.write().await;
            // Check the lease again under the write lock. A publish renews the
            // lease before it takes this lock, so a publish that arrived after
            // the first check is visible here, and its snapshot stays.
            let renewed_at = event.renewed_at.load(Ordering::Relaxed);
            if inner.latest.is_none() || now.saturating_sub(renewed_at) < lease_secs {
                continue;
            }
            inner.latest = None;
            inner.seq += 1;
            let snapshot = MetadataSnapshot {
                event_id: event_id.clone(),
                seq: inner.seq,
                updated_at: format_timestamp(now),
                metadata: json!({}),
            };
            inner.replay.push_back(snapshot.clone());
            while inner.replay.len() > REPLAY_CAPACITY {
                inner.replay.pop_front();
            }
            drop(inner);

            let _ = event.sender.send(snapshot);
            self.emit_socket_remote_value(&event_id, &json!({})).await;
            expired_count += 1;
        }

        expired_count
    }

    fn attach_socket_io(&self, io: SocketIo) {
        self.inner
            .socket_io
            .set(io)
            .expect("socket_io already attached; app(state) called twice on same RelayState");
    }

    async fn emit_socket_remote_value(&self, event_id: &str, metadata: &Value) {
        if let Some(io) = self.inner.socket_io.get()
            && let Some(event_ns) = io.of("/event")
            && let Err(err) = event_ns
                .to(event_id.to_string())
                .emit("remoteValue", metadata)
                .await
        {
            tracing::warn!(%event_id, ?err, "failed to emit Socket.IO remoteValue");
        }
    }
}

fn try_acquire_window_slot(window: &AtomicU64, now: u64, limit: u32) -> bool {
    if limit == 0 {
        return true;
    }
    let now_window = now & 0xFFFF_FFFF;
    loop {
        let current = window.load(Ordering::Acquire);
        let stored_window = current >> 32;
        let count = (current & 0xFFFF_FFFF) as u32;
        let new = if stored_window == now_window {
            if count >= limit {
                return false;
            }
            (now_window << 32) | u64::from(count + 1)
        } else {
            (now_window << 32) | 1
        };
        if window
            .compare_exchange_weak(current, new, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return true;
        }
    }
}

fn unique_event_id(events: &HashMap<String, Arc<LiveEventState>>) -> String {
    loop {
        let event_id = random_urlsafe(EVENT_ID_BYTES);
        if !events.contains_key(&event_id) {
            return event_id;
        }
    }
}

struct LiveEventState {
    token_hash: [u8; 32],
    last_activity: AtomicU64,
    renewed_at: AtomicU64,
    publish_window: AtomicU64,
    inner: RwLock<LiveEventInner>,
    sender: broadcast::Sender<MetadataSnapshot>,
}

struct LiveEventInner {
    seq: u64,
    latest: Option<MetadataSnapshot>,
    replay: VecDeque<MetadataSnapshot>,
}

impl LiveEventState {
    fn new(token_hash: [u8; 32], now: u64) -> Self {
        let (sender, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            token_hash,
            last_activity: AtomicU64::new(now),
            renewed_at: AtomicU64::new(now),
            publish_window: AtomicU64::new(0),
            inner: RwLock::new(LiveEventInner {
                seq: 0,
                latest: None,
                replay: VecDeque::with_capacity(REPLAY_CAPACITY),
            }),
            sender,
        }
    }

    fn try_acquire_publish_slot(&self, now: u64, limit: u32) -> bool {
        try_acquire_window_slot(&self.publish_window, now, limit)
    }

    fn token_matches(&self, token: &str) -> bool {
        let candidate = hash_token(token);
        self.token_hash
            .as_slice()
            .ct_eq(candidate.as_slice())
            .into()
    }

    fn touch(&self, now: u64) {
        self.last_activity.store(now, Ordering::Relaxed);
    }

    fn renew_lease(&self, now: u64) {
        self.renewed_at.store(now, Ordering::Relaxed);
    }
}

struct EventSubscription {
    state: RelayState,
    event: Arc<LiveEventState>,
    replay: Vec<MetadataSnapshot>,
    receiver: broadcast::Receiver<MetadataSnapshot>,
}

impl Drop for EventSubscription {
    fn drop(&mut self) {
        self.state
            .inner
            .sse_connections
            .fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
pub struct PublishMetadataRequest(Value);

impl PublishMetadataRequest {
    fn into_metadata(self, path_event_id: &str) -> Result<Value, ApiError> {
        if let Some(map) = self.0.as_object()
            && map.len() == 2
            && map.contains_key("event_id")
            && map.contains_key("metadata")
        {
            let event_id = map
                .get("event_id")
                .and_then(Value::as_str)
                .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "invalid_event_id"))?;
            if event_id != path_event_id {
                return Err(ApiError::new(StatusCode::BAD_REQUEST, "event_id_mismatch"));
            }
            let mut owned = self.0;
            let metadata = owned
                .as_object_mut()
                .and_then(|m| m.remove("metadata"))
                .expect("metadata key present");
            return Ok(metadata);
        }
        Ok(self.0)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateEventResponse {
    pub event_id: String,
    pub broadcaster_token: String,
    pub metadata_url: String,
    pub remote_value_url: String,
    pub events_url: String,
    pub socket_io_url: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PublishMetadataResponse {
    pub event_id: String,
    pub accepted: bool,
    pub seq: u64,
    pub lease_secs: u64,
    pub keepalive_interval_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestMetadataResponse {
    pub event_id: String,
    pub seq: u64,
    pub updated_at: String,
    pub renewed_at: String,
    pub lease_expires_at: String,
    pub metadata: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct KeepaliveResponse {
    pub event_id: String,
    pub lease_expires_at: String,
    pub keepalive_interval_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MetadataSnapshot {
    event_id: String,
    seq: u64,
    updated_at: String,
    metadata: Value,
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str) -> Self {
        Self { status, code }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({
                "error": self.code,
            })),
        )
            .into_response()
    }
}

pub fn app(state: RelayState) -> Router {
    let (socket_layer, io) = SocketIo::builder().with_state(state.clone()).build_layer();

    register_socket_namespaces(io.clone());
    state.attach_socket_io(io);

    Router::new()
        .route("/health", get(health))
        .route("/v1/liveitems/health", get(health))
        .route("/v1/liveitems", post(create_event))
        .route("/v1/liveitems/", post(create_event))
        .route(
            "/v1/liveitems/{event_id}/metadata",
            get(latest_metadata)
                .post(publish_metadata)
                .route_layer(RequestBodyLimitLayer::new(METADATA_BODY_LIMIT_BYTES)),
        )
        .route("/v1/liveitems/{event_id}/remoteValue", get(remote_value))
        .route("/v1/liveitems/{event_id}/events", get(events))
        .route("/v1/liveitems/{event_id}/keepalive", post(keepalive_route))
        .layer(CorsLayer::permissive())
        .with_state(state)
        .layer(socket_layer)
}

pub fn spawn_cleanup_task(state: RelayState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let interval = std::cmp::min(state.inner.config.ttl / 4, Duration::from_secs(60 * 60));
        let interval = std::cmp::max(interval, Duration::from_secs(60));
        let mut ticker = tokio::time::interval(interval);

        loop {
            ticker.tick().await;
            let expired = state.cleanup_expired().await;
            if expired > 0 {
                tracing::info!(expired, "expired inactive live items");
            }
        }
    })
}

/// Spawns the background task that expires leases each second.
///
/// The task calls [`RelayState::expire_leases`] on a one-second tick for the
/// life of the process.
pub fn spawn_lease_task(state: RelayState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));

        loop {
            ticker.tick().await;
            let expired = state.expire_leases().await;
            if expired > 0 {
                tracing::info!(expired, "expired live leases");
            }
        }
    })
}

async fn health() -> &'static str {
    "ok"
}

async fn create_event(
    AxumState(state): AxumState<RelayState>,
) -> Result<Json<CreateEventResponse>, ApiError> {
    Ok(Json(state.create_event().await?))
}

async fn publish_metadata(
    AxumState(state): AxumState<RelayState>,
    Path(event_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<PublishMetadataRequest>,
) -> Result<Json<PublishMetadataResponse>, ApiError> {
    let token = bearer_token(&headers)?;
    Ok(Json(state.publish_metadata(&event_id, token, body).await?))
}

async fn latest_metadata(
    AxumState(state): AxumState<RelayState>,
    Path(event_id): Path<String>,
) -> Result<Json<LatestMetadataResponse>, ApiError> {
    Ok(Json(state.latest_metadata(&event_id).await?))
}

async fn keepalive_route(
    AxumState(state): AxumState<RelayState>,
    Path(event_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<KeepaliveResponse>, ApiError> {
    let token = bearer_token(&headers)?;
    Ok(Json(state.keepalive(&event_id, token).await?))
}

async fn remote_value(
    AxumState(state): AxumState<RelayState>,
    Path(event_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let event = state.get_event(&event_id).await?;
    let metadata = event
        .inner
        .read()
        .await
        .latest
        .as_ref()
        .map(|snapshot| snapshot.metadata.clone())
        .unwrap_or_else(|| json!({}));
    Ok(Json(metadata))
}

async fn events(
    AxumState(state): AxumState<RelayState>,
    Path(event_id): Path<String>,
    headers: HeaderMap,
) -> Result<Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let last_event_id = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());

    let subscription = state.subscribe(&event_id, last_event_id).await?;

    let stream = async_stream::stream! {
        let mut subscription = subscription;

        for snapshot in subscription.replay.drain(..) {
            yield Ok(snapshot_to_sse_remote_value(snapshot));
        }

        loop {
            match subscription.receiver.recv().await {
                Ok(snapshot) => {
                    subscription.event.touch(subscription.state.now());
                    yield Ok(snapshot_to_sse_remote_value(snapshot));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

fn register_socket_namespaces(io: SocketIo) {
    io.ns(
        "/event",
        async |socket: SocketRef, SocketState(state): SocketState<RelayState>| {
            let Some(event_id) = socket_event_id(&socket) else {
                let _ = socket.emit("remoteValue", &json!({}));
                socket.disconnect().ok();
                return;
            };

            let event = match state.get_event(&event_id).await {
                Ok(event) => event,
                Err(_) => {
                    let _ = socket.emit("remoteValue", &json!({}));
                    socket.disconnect().ok();
                    return;
                }
            };

            socket.join(event_id.clone());

            let current = event
                .inner
                .read()
                .await
                .latest
                .as_ref()
                .map(|snapshot| snapshot.metadata.clone())
                .unwrap_or_else(|| json!({}));

            if let Err(err) = socket.emit("remoteValue", &current) {
                tracing::warn!(%event_id, ?err, "failed to emit initial Socket.IO remoteValue");
            }
        },
    );
}

fn socket_event_id(socket: &SocketRef) -> Option<String> {
    socket.req_parts().uri.query()?.split('&').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        (key == "event_id" && !value.is_empty()).then(|| value.to_string())
    })
}

fn snapshot_to_sse_remote_value(snapshot: MetadataSnapshot) -> Event {
    Event::default()
        .event("remoteValue")
        .id(snapshot.seq.to_string())
        .json_data(snapshot.metadata)
        .expect("metadata payloads are serializable")
}

fn bearer_token(headers: &HeaderMap) -> Result<&str, ApiError> {
    let value = headers
        .get(header::AUTHORIZATION)
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "missing_bearer_token"))?
        .to_str()
        .map_err(|_| ApiError::new(StatusCode::UNAUTHORIZED, "invalid_authorization_header"))?;

    let (scheme, token) = value
        .split_once(' ')
        .ok_or_else(|| ApiError::new(StatusCode::UNAUTHORIZED, "invalid_bearer_token"))?;

    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_bearer_token",
        ));
    }

    Ok(token)
}

fn hash_token(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn random_urlsafe(byte_count: usize) -> String {
    let mut bytes = vec![0; byte_count];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(&bytes)
}

fn epoch_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_secs()
}

fn format_timestamp(timestamp: u64) -> String {
    OffsetDateTime::from_unix_timestamp(timestamp.try_into().expect("timestamp fits in i64"))
        .expect("timestamp is in range")
        .format(&Rfc3339)
        .expect("timestamp can be formatted as RFC 3339")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    fn test_state(now: u64) -> (RelayState, Arc<AtomicU64>) {
        test_state_with_lease(now, DEFAULT_LEASE_SECS)
    }

    fn test_state_with_lease(now: u64, lease_secs: u64) -> (RelayState, Arc<AtomicU64>) {
        let clock = Arc::new(AtomicU64::new(now));
        let state = RelayState::with_clock(
            AppConfig {
                bind: DEFAULT_BIND.parse().expect("default bind is valid"),
                max_active_events: DEFAULT_MAX_ACTIVE_EVENTS,
                max_sse_connections: DEFAULT_MAX_SSE_CONNECTIONS,
                ttl: Duration::from_secs(10),
                max_publishes_per_event_per_sec: DEFAULT_MAX_PUBLISHES_PER_EVENT_PER_SEC,
                max_creates_per_sec: DEFAULT_MAX_CREATES_PER_SEC,
                lease: Duration::from_secs(lease_secs),
            },
            {
                let clock = clock.clone();
                Arc::new(move || clock.load(Ordering::SeqCst))
            },
        );
        (state, clock)
    }

    #[tokio::test]
    async fn event_creation_returns_unique_event_id_and_token() {
        let (state, _) = test_state(1);
        let first = state.create_event().await.expect("create first event");
        let second = state.create_event().await.expect("create second event");

        assert_ne!(first.event_id, second.event_id);
        assert_ne!(first.broadcaster_token, second.broadcaster_token);
        assert!(first.metadata_url.ends_with("/metadata"));
        assert!(first.remote_value_url.ends_with("/remoteValue"));
        assert!(first.events_url.ends_with("/events"));
        assert!(first.socket_io_url.starts_with("/event?event_id="));
    }

    #[tokio::test]
    async fn token_hash_validation_accepts_correct_token_and_rejects_wrong_token() {
        let (state, _) = test_state(1);
        let created = state.create_event().await.expect("create event");

        state
            .publish_metadata(
                &created.event_id,
                &created.broadcaster_token,
                PublishMetadataRequest(json!({
                    "event_id": created.event_id.clone(),
                    "metadata": {"title": "First"},
                })),
            )
            .await
            .expect("valid token publishes");

        let err = state
            .publish_metadata(
                &created.event_id,
                "wrong",
                PublishMetadataRequest(json!({
                    "event_id": created.event_id.clone(),
                    "metadata": {"title": "Second"},
                })),
            )
            .await
            .expect_err("wrong token rejected");
        assert_eq!(err.status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn path_body_event_id_mismatch_returns_400() {
        let (state, _) = test_state(1);
        let created = state.create_event().await.expect("create event");
        let err = state
            .publish_metadata(
                &created.event_id,
                &created.broadcaster_token,
                PublishMetadataRequest(json!({
                    "event_id": "different",
                    "metadata": {},
                })),
            )
            .await
            .expect_err("mismatch rejected");

        assert_eq!(err.status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn latest_snapshot_returns_most_recent_metadata() {
        let (state, _) = test_state(1);
        let created = state.create_event().await.expect("create event");

        for title in ["First", "Second"] {
            state
                .publish_metadata(
                    &created.event_id,
                    &created.broadcaster_token,
                    PublishMetadataRequest(json!({
                        "event_id": created.event_id.clone(),
                        "metadata": {"title": title},
                    })),
                )
                .await
                .expect("publish");
        }

        let latest = state
            .latest_metadata(&created.event_id)
            .await
            .expect("latest metadata");
        assert_eq!(latest.seq, 2);
        assert_eq!(latest.metadata, json!({"title": "Second"}));
    }

    #[tokio::test]
    async fn replay_buffer_is_bounded_to_100_events() {
        let (state, clock) = test_state(1);
        let created = state.create_event().await.expect("create event");

        for index in 0_u64..105 {
            // Advance clock past per-event publish rate window each iteration.
            clock.store(2 + index, Ordering::SeqCst);
            state
                .publish_metadata(
                    &created.event_id,
                    &created.broadcaster_token,
                    PublishMetadataRequest(json!({
                        "event_id": created.event_id.clone(),
                        "metadata": {"index": index},
                    })),
                )
                .await
                .expect("publish");
        }

        let event = state.get_event(&created.event_id).await.expect("event");
        let inner = event.inner.read().await;
        assert_eq!(inner.replay.len(), REPLAY_CAPACITY);
        assert_eq!(inner.replay.front().expect("front").seq, 6);
        assert_eq!(inner.replay.back().expect("back").seq, 105);
    }

    #[tokio::test]
    async fn subscribe_touches_last_activity_so_event_is_not_evicted() {
        let (state, clock) = test_state(100);
        let created = state.create_event().await.expect("create event");

        clock.store(108, Ordering::SeqCst);
        let _subscription = state
            .subscribe(&created.event_id, None)
            .await
            .expect("subscribe");

        clock.store(115, Ordering::SeqCst); // cutoff = 105; touched at 108 → kept
        assert_eq!(state.cleanup_expired().await, 0);
        state
            .get_event(&created.event_id)
            .await
            .expect("event still present");
    }

    #[tokio::test]
    async fn direct_payload_with_event_id_and_metadata_keys_is_treated_as_direct() {
        let (state, _) = test_state(1);
        let created = state.create_event().await.expect("create event");

        let payload = json!({
            "event_id": "different-from-path",
            "metadata": {"x": 1},
            "extra": "field",
        });

        state
            .publish_metadata(
                &created.event_id,
                &created.broadcaster_token,
                PublishMetadataRequest(payload.clone()),
            )
            .await
            .expect("direct payload accepted");

        let latest = state
            .latest_metadata(&created.event_id)
            .await
            .expect("latest");
        assert_eq!(latest.metadata, payload);
    }

    #[tokio::test]
    async fn publish_rate_limit_returns_429_when_exceeded() {
        let clock = Arc::new(AtomicU64::new(1));
        let state = RelayState::with_clock(
            AppConfig {
                max_publishes_per_event_per_sec: 2,
                ..AppConfig::for_tests()
            },
            {
                let clock = clock.clone();
                Arc::new(move || clock.load(Ordering::SeqCst))
            },
        );
        let created = state.create_event().await.expect("create event");

        for _ in 0..2 {
            state
                .publish_metadata(
                    &created.event_id,
                    &created.broadcaster_token,
                    PublishMetadataRequest(json!({"x": 1})),
                )
                .await
                .expect("publish under limit");
        }

        let err = state
            .publish_metadata(
                &created.event_id,
                &created.broadcaster_token,
                PublishMetadataRequest(json!({"x": 1})),
            )
            .await
            .expect_err("over limit");
        assert_eq!(err.status, StatusCode::TOO_MANY_REQUESTS);

        clock.store(2, Ordering::SeqCst);
        state
            .publish_metadata(
                &created.event_id,
                &created.broadcaster_token,
                PublishMetadataRequest(json!({"x": 1})),
            )
            .await
            .expect("next-window publish accepted");
    }

    #[tokio::test]
    async fn create_rate_limit_returns_429_when_exceeded() {
        let clock = Arc::new(AtomicU64::new(1));
        let state = RelayState::with_clock(
            AppConfig {
                max_creates_per_sec: 2,
                ..AppConfig::for_tests()
            },
            {
                let clock = clock.clone();
                Arc::new(move || clock.load(Ordering::SeqCst))
            },
        );

        state.create_event().await.expect("first");
        state.create_event().await.expect("second");
        let err = state.create_event().await.expect_err("third");
        assert_eq!(err.status, StatusCode::TOO_MANY_REQUESTS);

        clock.store(2, Ordering::SeqCst);
        state.create_event().await.expect("next-window create");
    }

    #[tokio::test]
    async fn max_active_events_returns_503_when_full() {
        let clock = Arc::new(AtomicU64::new(1));
        let state = RelayState::with_clock(
            AppConfig {
                max_active_events: 2,
                ..AppConfig::for_tests()
            },
            {
                let clock = clock.clone();
                Arc::new(move || clock.load(Ordering::SeqCst))
            },
        );
        state.create_event().await.expect("first");
        state.create_event().await.expect("second");
        let err = state.create_event().await.expect_err("third");
        assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(err.code, "max_active_events_reached");
    }

    #[tokio::test]
    async fn max_sse_connections_enforced_and_decrements_on_drop() {
        let clock = Arc::new(AtomicU64::new(1));
        let state = RelayState::with_clock(
            AppConfig {
                max_sse_connections: 1,
                ..AppConfig::for_tests()
            },
            {
                let clock = clock.clone();
                Arc::new(move || clock.load(Ordering::SeqCst))
            },
        );
        let created = state.create_event().await.expect("create");

        let sub = state
            .subscribe(&created.event_id, None)
            .await
            .expect("first subscribe");

        let err = match state.subscribe(&created.event_id, None).await {
            Ok(_) => panic!("second subscribe should be rejected"),
            Err(err) => err,
        };
        assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(err.code, "max_sse_connections_reached");

        drop(sub);
        state
            .subscribe(&created.event_id, None)
            .await
            .map(|_| ())
            .expect("post-drop subscribe");
    }

    #[tokio::test]
    async fn ttl_cleanup_removes_inactive_events() {
        let (state, clock) = test_state(100);
        let created = state.create_event().await.expect("create event");

        clock.store(111, Ordering::SeqCst);

        assert_eq!(state.cleanup_expired().await, 1);
        let err = match state.get_event(&created.event_id).await {
            Ok(_) => panic!("event expired"),
            Err(err) => err,
        };
        assert_eq!(err.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn publish_response_reports_default_lease_and_keepalive_interval() {
        let (state, _) = test_state(1);
        let created = state.create_event().await.expect("create event");

        let response = state
            .publish_metadata(
                &created.event_id,
                &created.broadcaster_token,
                PublishMetadataRequest(json!({"title": "Live"})),
            )
            .await
            .expect("publish");

        assert_eq!(response.lease_secs, DEFAULT_LEASE_SECS);
        assert_eq!(response.keepalive_interval_secs, DEFAULT_LEASE_SECS / 3);
    }

    #[tokio::test]
    async fn expire_leases_does_not_clear_a_snapshot_before_the_lease_duration() {
        let (state, clock) = test_state_with_lease(1, 10);
        let created = state.create_event().await.expect("create event");
        state
            .publish_metadata(
                &created.event_id,
                &created.broadcaster_token,
                PublishMetadataRequest(json!({"title": "Live"})),
            )
            .await
            .expect("publish");

        clock.store(10, Ordering::SeqCst); // renewed_at=1, now-renewed_at=9 < 10
        assert_eq!(state.expire_leases().await, 0);

        let latest = state
            .latest_metadata(&created.event_id)
            .await
            .expect("snapshot still present");
        assert_eq!(latest.metadata, json!({"title": "Live"}));
    }

    #[tokio::test]
    async fn expire_leases_clears_the_snapshot_at_the_lease_duration() {
        let (state, clock) = test_state_with_lease(1, 10);
        let created = state.create_event().await.expect("create event");
        state
            .publish_metadata(
                &created.event_id,
                &created.broadcaster_token,
                PublishMetadataRequest(json!({"title": "Live"})),
            )
            .await
            .expect("publish");

        clock.store(11, Ordering::SeqCst); // renewed_at=1, now-renewed_at=10 >= 10
        assert_eq!(state.expire_leases().await, 1);

        let event = state.get_event(&created.event_id).await.expect("event");
        let inner = event.inner.read().await;
        assert!(inner.latest.is_none(), "snapshot should be cleared");
        drop(inner);

        let err = state
            .latest_metadata(&created.event_id)
            .await
            .expect_err("expired event has no metadata");
        assert_eq!(err.status, StatusCode::NOT_FOUND);
        assert_eq!(err.code, "metadata_not_found");
    }

    #[tokio::test]
    async fn sse_reconnect_after_expiry_receives_empty_object() {
        let (state, clock) = test_state_with_lease(1, 10);
        let created = state.create_event().await.expect("create event");
        let published = state
            .publish_metadata(
                &created.event_id,
                &created.broadcaster_token,
                PublishMetadataRequest(json!({"title": "Live"})),
            )
            .await
            .expect("publish");

        clock.store(11, Ordering::SeqCst);
        assert_eq!(state.expire_leases().await, 1);

        let subscription = state
            .subscribe(&created.event_id, Some(published.seq))
            .await
            .expect("subscribe with last-event-id before the expiry");

        assert_eq!(subscription.replay.len(), 1);
        let replayed = &subscription.replay[0];
        assert_eq!(replayed.seq, published.seq + 1);
        assert_eq!(replayed.metadata, json!({}));
    }

    #[tokio::test]
    async fn publish_after_expiry_brings_the_event_back_on_air() {
        let (state, clock) = test_state_with_lease(1, 10);
        let created = state.create_event().await.expect("create event");
        state
            .publish_metadata(
                &created.event_id,
                &created.broadcaster_token,
                PublishMetadataRequest(json!({"title": "First"})),
            )
            .await
            .expect("publish");

        clock.store(11, Ordering::SeqCst);
        assert_eq!(state.expire_leases().await, 1);

        clock.store(12, Ordering::SeqCst);
        state
            .publish_metadata(
                &created.event_id,
                &created.broadcaster_token,
                PublishMetadataRequest(json!({"title": "Back"})),
            )
            .await
            .expect("publish after expiry");

        let latest = state
            .latest_metadata(&created.event_id)
            .await
            .expect("event is on air again");
        assert_eq!(latest.metadata, json!({"title": "Back"}));
    }

    #[tokio::test]
    async fn expire_leases_does_not_change_an_event_with_no_snapshot() {
        let (state, clock) = test_state_with_lease(1, 10);
        let created = state.create_event().await.expect("create event");

        clock.store(1_000, Ordering::SeqCst);
        assert_eq!(state.expire_leases().await, 0);

        let event = state.get_event(&created.event_id).await.expect("event");
        assert_eq!(event.last_activity.load(Ordering::Relaxed), 1);
        assert!(event.inner.read().await.latest.is_none());
    }

    #[test]
    fn lease_secs_below_minimum_is_a_configuration_error() {
        // SAFETY: this test is the only one in the suite that reads or
        // writes LEASE_SECS, so no other test races this mutation of the
        // process environment.
        unsafe {
            std::env::set_var("LEASE_SECS", "5");
        }
        let result = AppConfig::from_env();
        // SAFETY: see above.
        unsafe {
            std::env::remove_var("LEASE_SECS");
        }

        let err = result.expect_err("a lease below the minimum is rejected");
        assert!(err.to_string().contains("LEASE_SECS"));
    }
}
