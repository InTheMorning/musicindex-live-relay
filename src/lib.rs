pub mod store;

use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    fmt::Write as _,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock, PoisonError,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State as AxumState, rejection::BytesRejection},
    http::{HeaderMap, StatusCode, header},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{delete, get, post},
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
use tokio::sync::{RwLock, broadcast, watch};
use tower_http::{cors::CorsLayer, limit::RequestBodyLimitLayer};

use crate::store::{
    EventClass, EventStore, MemoryEventStore, SqliteEventStore, StoreError, StoredEvent,
};

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
const DEFAULT_STATE_FILE: &str = "/var/lib/musicindex-live-relay/reserved-items.sqlite3";
const DEFAULT_MAX_RESERVED_ITEMS: usize = 100;
const MIN_ADMIN_TOKEN_BYTES: usize = 16;
const RESERVE_BODY_LIMIT_BYTES: usize = 4 * 1024;
const MAX_LABEL_CHARS: usize = 200;
/// The body limit of a display publish (ADR 0003).
const DISPLAY_BODY_LIMIT_BYTES: usize = 8 * 1024;
/// The maximum length of an `artwork.url`, in characters (ADR 0003).
const MAX_ARTWORK_URL_CHARS: usize = 2_048;
/// The image types that an `artwork.mime` can name (ADR 0003).
const ARTWORK_MIME_TYPES: [&str; 2] = ["image/jpeg", "image/png"];
/// The default body limit of an image upload, in bytes (ADR 0003).
const DEFAULT_ARTWORK_MAX_BYTES: usize = 512 * 1024;
/// The maximum count of images that one event holds (ADR 0003 §Invariants).
const MAX_EVENT_IMAGES: usize = 2;
/// The first bytes of a JPEG image.
const JPEG_MAGIC: &[u8] = &[0xFF, 0xD8, 0xFF];
/// The first bytes of a PNG image.
const PNG_MAGIC: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
/// The `Cache-Control` value of an image read. The path holds the hash of
/// the bytes, so the bytes of a path never change.
const ARTWORK_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// The operator credential for the reserved item routes (ADR 0001).
///
/// It holds the SHA-256 hash of the token, not the token. Its `Debug` output
/// shows no part of the hash.
#[derive(Clone)]
pub struct AdminToken {
    hash: [u8; 32],
}

impl AdminToken {
    /// Makes an admin credential from the configured token.
    pub fn new(token: &str) -> Self {
        Self {
            hash: hash_token(token),
        }
    }

    /// Compares `candidate` with the admin token in constant time.
    fn matches(&self, candidate: &str) -> bool {
        self.hash
            .as_slice()
            .ct_eq(hash_token(candidate).as_slice())
            .into()
    }
}

impl std::fmt::Debug for AdminToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AdminToken(<redacted>)")
    }
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub bind: SocketAddr,
    pub max_active_events: usize,
    pub max_sse_connections: usize,
    pub ttl: Duration,
    pub max_publishes_per_event_per_sec: u32,
    pub max_creates_per_sec: u32,
    pub lease: Duration,
    /// The admin credential. With no value, the reserved item routes answer
    /// `404`, and the relay opens no state file.
    pub admin_token: Option<AdminToken>,
    /// The SQLite file that holds the reserved items.
    pub state_file: PathBuf,
    /// The maximum count of reserved items in the state file.
    pub max_reserved_items: usize,
    /// The body limit of an image upload, in bytes (ADR 0003).
    pub artwork_max_bytes: usize,
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
            admin_token: parse_admin_token()?,
            state_file: env_parse("STATE_FILE", PathBuf::from(DEFAULT_STATE_FILE))?,
            max_reserved_items: env_parse("MAX_RESERVED_ITEMS", DEFAULT_MAX_RESERVED_ITEMS)?,
            artwork_max_bytes: parse_artwork_max_bytes()?,
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
            admin_token: None,
            state_file: PathBuf::from(DEFAULT_STATE_FILE),
            max_reserved_items: DEFAULT_MAX_RESERVED_ITEMS,
            artwork_max_bytes: DEFAULT_ARTWORK_MAX_BYTES,
        }
    }
}

/// Reads `ADMIN_TOKEN`. An error never holds the value.
fn parse_admin_token() -> Result<Option<AdminToken>, ConfigError> {
    let redacted = |message: String| ConfigError {
        key: "ADMIN_TOKEN",
        value: "<redacted>".to_string(),
        message,
    };
    match std::env::var("ADMIN_TOKEN") {
        Ok(token) if token.len() < MIN_ADMIN_TOKEN_BYTES => Err(redacted(format!(
            "must be {MIN_ADMIN_TOKEN_BYTES} bytes or more"
        ))),
        Ok(token) => Ok(Some(AdminToken::new(&token))),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(redacted("not valid unicode".to_string())),
    }
}

/// Reads `ARTWORK_MAX_BYTES`, the body limit of an image upload.
fn parse_artwork_max_bytes() -> Result<usize, ConfigError> {
    env_parse("ARTWORK_MAX_BYTES", DEFAULT_ARTWORK_MAX_BYTES)
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
    // The `Arc` lets a reserve move an owned write guard into a blocking task.
    events: Arc<RwLock<EventTable>>,
    sse_connections: AtomicUsize,
    create_window: AtomicU64,
    socket_io: OnceLock<SocketIo>,
    clock: Clock,
}

/// The identity half and the live half of each event.
///
/// One lock protects both halves, so they always hold the same identifiers.
/// The store holds the identity. `live` holds the snapshot, the replay
/// buffer, the broadcast sender, and the lease time.
struct EventTable {
    store: Box<dyn EventStore>,
    live: HashMap<String, Arc<LiveEventState>>,
}

type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

impl RelayState {
    /// Makes the relay state.
    ///
    /// # Panics
    ///
    /// Panics when an admin token is configured and the state file cannot be
    /// opened or restored. Use [`RelayState::try_new`] to get the error.
    pub fn new(config: AppConfig) -> Self {
        Self::with_clock(config, Arc::new(epoch_seconds))
    }

    /// Makes the relay state with an injected time source.
    ///
    /// # Panics
    ///
    /// Panics when an admin token is configured and the state file cannot be
    /// opened or restored. Use [`RelayState::try_with_clock`] to get the error.
    pub fn with_clock(config: AppConfig, clock: Clock) -> Self {
        Self::try_with_clock(config, clock).expect("open the state file")
    }

    /// Makes the relay state.
    ///
    /// With an admin token configured, the relay opens the state file,
    /// restores each reserved item from it, and writes each new reserved item
    /// to it. With no admin token, the relay opens no file and keeps each item
    /// in memory only.
    ///
    /// # Errors
    ///
    /// Returns a `StoreError` when the state file cannot be opened or read,
    /// or when it is corrupt. The error names the path.
    pub fn try_new(config: AppConfig) -> Result<Self, StoreError> {
        Self::try_with_clock(config, Arc::new(epoch_seconds))
    }

    /// Makes the relay state with an injected time source.
    ///
    /// A restored item gets new live state at the current time: the stored
    /// token hash, no snapshot, an empty replay buffer, and `seq` at zero
    /// (ADR 0001). It is not on air until the next publish.
    ///
    /// # Errors
    ///
    /// Returns a `StoreError` when the state file cannot be opened or read,
    /// or when it is corrupt. The error names the path.
    pub fn try_with_clock(config: AppConfig, clock: Clock) -> Result<Self, StoreError> {
        let mut live = HashMap::new();
        let store: Box<dyn EventStore> = if config.admin_token.is_some() {
            let mut store = SqliteEventStore::open(&config.state_file, config.max_reserved_items)?;
            let restored = store.load()?;
            let now = clock();
            for event in &restored {
                live.insert(event.event_id.clone(), Arc::new(LiveEventState::new(now)));
            }
            // Log the path and the count only. No identifier and no hash.
            tracing::info!(
                state_file = %config.state_file.display(),
                restored = restored.len(),
                "restored reserved live items"
            );
            Box::new(store)
        } else {
            Box::new(MemoryEventStore::new())
        };
        Ok(Self {
            inner: Arc::new(RelayStateInner {
                config,
                events: Arc::new(RwLock::new(EventTable { store, live })),
                sse_connections: AtomicUsize::new(0),
                create_window: AtomicU64::new(0),
                socket_io: OnceLock::new(),
                clock,
            }),
        })
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
        if events.live.len() >= self.inner.config.max_active_events {
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "max_active_events_reached",
            ));
        }

        let event_id = unique_event_id(events.store.as_ref());
        let token = random_urlsafe(TOKEN_BYTES);
        let now = self.now();

        events.store.insert(StoredEvent {
            event_id: event_id.clone(),
            token_hash: hash_token(&token),
            label: None,
            created_at: now,
            class: EventClass::Ephemeral,
        })?;
        events
            .live
            .insert(event_id.clone(), Arc::new(LiveEventState::new(now)));

        Ok(CreateEventResponse {
            event_id: event_id.clone(),
            broadcaster_token: token,
            metadata_url: format!("/v1/liveitems/{event_id}/metadata"),
            remote_value_url: format!("/v1/liveitems/{event_id}/remoteValue"),
            events_url: format!("/v1/liveitems/{event_id}/events"),
            socket_io_url: format!("/event?event_id={event_id}"),
        })
    }

    /// Checks the admin credential of a request to a reserved item route.
    ///
    /// # Errors
    ///
    /// Returns `404 reserved_items_disabled` when no admin token is
    /// configured, `401` when the credential is missing or malformed, and
    /// `403 invalid_admin_token` when the credential is wrong.
    fn authorize_admin(&self, headers: &HeaderMap) -> Result<(), ApiError> {
        let Some(admin_token) = &self.inner.config.admin_token else {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "reserved_items_disabled",
            ));
        };
        let candidate = bearer_token(headers)?;
        if !admin_token.matches(candidate) {
            return Err(ApiError::new(StatusCode::FORBIDDEN, "invalid_admin_token"));
        }
        Ok(())
    }

    /// Reserves a live item with a durable identity (ADR 0001).
    ///
    /// The store writes the identity and the token hash to the state file in
    /// one transaction. The SQLite call blocks, so it runs on the blocking
    /// thread pool. The table write lock stays held for that call, so the
    /// label check, the count check, and the insert are one step. No event
    /// lock is held, and no subscriber is waited on.
    ///
    /// # Errors
    ///
    /// Returns `409 label_already_reserved`,
    /// `503 max_reserved_items_reached`, or `500 store_unavailable`.
    pub async fn reserve_event(
        &self,
        label: Option<String>,
    ) -> Result<ReserveEventResponse, ApiError> {
        let token = random_urlsafe(TOKEN_BYTES);
        let now = self.now();

        let mut events = Arc::clone(&self.inner.events).write_owned().await;
        let event_id = unique_event_id(events.store.as_ref());
        let stored = StoredEvent {
            event_id: event_id.clone(),
            token_hash: hash_token(&token),
            label: label.clone(),
            created_at: now,
            class: EventClass::Reserved,
        };
        let (mut events, result) = tokio::task::spawn_blocking(move || {
            let result = events.store.insert(stored);
            (events, result)
        })
        .await
        .map_err(|err| {
            tracing::error!(error = %err, "reserve task failed");
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_unavailable")
        })?;
        result?;
        events
            .live
            .insert(event_id.clone(), Arc::new(LiveEventState::new(now)));
        drop(events);

        Ok(ReserveEventResponse {
            event_id: event_id.clone(),
            broadcaster_token: token,
            metadata_url: format!("/v1/liveitems/{event_id}/metadata"),
            remote_value_url: format!("/v1/liveitems/{event_id}/remoteValue"),
            events_url: format!("/v1/liveitems/{event_id}/events"),
            socket_io_url: format!("/event?event_id={event_id}"),
            label,
        })
    }

    /// Returns the reserved items, oldest first (ADR 0001).
    ///
    /// The list holds no ephemeral item, no token, and no token hash. The
    /// table lock is released before the publish time of an item is read.
    pub async fn list_reserved(&self) -> ReservedItemsResponse {
        let mut entries: Vec<(StoredEvent, Option<Arc<LiveEventState>>)> = {
            let events = self.inner.events.read().await;
            events
                .store
                .list_ids()
                .into_iter()
                .filter_map(|event_id| {
                    let stored = events.store.get(&event_id)?;
                    (stored.class == EventClass::Reserved).then(|| {
                        let live = events.live.get(&event_id).cloned();
                        (stored, live)
                    })
                })
                .collect()
        };
        entries.sort_by(|(a, _), (b, _)| {
            (a.created_at, &a.event_id).cmp(&(b.created_at, &b.event_id))
        });

        ReservedItemsResponse {
            reserved: entries
                .into_iter()
                .map(|(stored, live)| ReservedItem {
                    created_at: format_timestamp(stored.created_at),
                    last_publish_at: live
                        .and_then(|live| live.last_publish_at())
                        .map(format_timestamp),
                    event_id: stored.event_id,
                    label: stored.label,
                })
                .collect(),
        }
    }

    /// Deletes a reserved item permanently (ADR 0001).
    ///
    /// The steps are in this order:
    ///
    /// 1. The store deletes the row from the state file. The SQLite call
    ///    blocks, so it runs on the blocking thread pool with the table write
    ///    lock held, as a reserve does.
    /// 2. The live state leaves memory. The table write lock is then
    ///    released.
    /// 3. Each SSE stream of the item stops.
    /// 4. Each Socket.IO client of the item gets `{}` and is disconnected.
    ///
    /// If step 1 fails, nothing changes. No lock is held during step 3 or
    /// step 4.
    ///
    /// # Errors
    ///
    /// Returns `404 event_not_found` when no reserved item holds `event_id`.
    /// An ephemeral item gives the same answer and stays unchanged. Returns
    /// `500 store_unavailable` when the state file write fails.
    pub async fn delete_reserved(&self, event_id: &str) -> Result<(), ApiError> {
        let events = Arc::clone(&self.inner.events).write_owned().await;
        let is_reserved = events
            .store
            .get(event_id)
            .is_some_and(|stored| stored.class == EventClass::Reserved);
        if !is_reserved {
            return Err(ApiError::new(StatusCode::NOT_FOUND, "event_not_found"));
        }

        let owned_event_id = event_id.to_string();
        let (mut events, result) = tokio::task::spawn_blocking(move || {
            let mut events = events;
            let result = events.store.remove(&owned_event_id);
            (events, result)
        })
        .await
        .map_err(|err| {
            tracing::error!(error = %err, "delete task failed");
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "store_unavailable")
        })?;
        result?;
        let live = events.live.remove(event_id);
        drop(events);

        if let Some(live) = live {
            live.close();
        }
        self.disconnect_socket_clients(event_id).await;
        tracing::info!(%event_id, "deleted reserved live item");
        Ok(())
    }

    /// Returns the stored identity and the live state of an event.
    ///
    /// The store decides if the event exists. The table lock is released
    /// before this function returns.
    async fn get_entry(
        &self,
        event_id: &str,
    ) -> Result<(StoredEvent, Arc<LiveEventState>), ApiError> {
        let events = self.inner.events.read().await;
        events
            .store
            .get(event_id)
            .zip(events.live.get(event_id).cloned())
            .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "event_not_found"))
    }

    async fn get_event(&self, event_id: &str) -> Result<Arc<LiveEventState>, ApiError> {
        self.get_entry(event_id).await.map(|(_, event)| event)
    }

    pub async fn publish_metadata(
        &self,
        event_id: &str,
        token: &str,
        body: PublishMetadataRequest,
    ) -> Result<PublishMetadataResponse, ApiError> {
        let (stored, event) = self.get_entry(event_id).await?;
        if !stored.token_hash_matches(&hash_token(token)) {
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
        event.record_publish(now);
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
        let (stored, event) = self.get_entry(event_id).await?;
        if !stored.token_hash_matches(&hash_token(token)) {
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

    /// Returns the stored identity and the live state of a reserved event.
    ///
    /// # Errors
    ///
    /// Returns `404 event_not_found` when the event does not exist, and
    /// `409 event_not_reserved` for an ephemeral event (ADR 0003).
    async fn get_reserved_entry(
        &self,
        event_id: &str,
    ) -> Result<(StoredEvent, Arc<LiveEventState>), ApiError> {
        let (stored, event) = self.get_entry(event_id).await?;
        if stored.class != EventClass::Reserved {
            return Err(ApiError::new(StatusCode::CONFLICT, "event_not_reserved"));
        }
        Ok((stored, event))
    }

    /// Publishes the display state of a reserved event (ADR 0003).
    ///
    /// The order of checks is: the event exists, the token matches, the
    /// event is reserved, the body was read in full, the body is a valid
    /// display state, and the publish rate limit allows the request. The
    /// handler gives `body` as the result of the body read, so a body error
    /// never comes before a credential error. A display publish does not renew the
    /// lease and does not change `renewed_at`.
    ///
    /// The last check runs under the display lock: an `artwork.sha256` must
    /// name an image that the event holds, with the same type. The same lock
    /// holds the images, so no upload and no lease expiry can remove that
    /// image between the check and the state change (ADR 0003).
    ///
    /// The display lock is the only lock held during the update. It is
    /// released before the update goes to the subscribers. The live value
    /// lock is never taken.
    ///
    /// # Errors
    ///
    /// Returns `404 event_not_found`, `403 invalid_token`,
    /// `409 event_not_reserved`, the error of `body`, `400 invalid_display`,
    /// `429 publish_rate_limited`, or `409 artwork_missing`. An
    /// `artwork.mime` that is not the type of the stored image gives
    /// `400 invalid_display`.
    pub async fn publish_display(
        &self,
        event_id: &str,
        token: &str,
        body: Result<Bytes, ApiError>,
    ) -> Result<PublishDisplayResponse, ApiError> {
        let (stored, event) = self.get_entry(event_id).await?;
        if !stored.token_hash_matches(&hash_token(token)) {
            return Err(ApiError::new(StatusCode::FORBIDDEN, "invalid_token"));
        }
        if stored.class != EventClass::Reserved {
            return Err(ApiError::new(StatusCode::CONFLICT, "event_not_reserved"));
        }
        let display = validate_display(&body?)?;

        let now = self.now();
        if !event.try_acquire_publish_slot(now, self.inner.config.max_publishes_per_event_per_sec) {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "publish_rate_limited",
            ));
        }
        event.touch(now);

        let mut inner = event.display.write().await;
        let update = inner.publish(display)?;
        drop(inner);
        let seq = update.seq;
        let _ = event.display_sender.send(update);

        Ok(PublishDisplayResponse {
            event_id: event_id.to_string(),
            accepted: true,
            seq,
        })
    }

    /// Returns the present display state of a reserved event, or
    /// `{"track": null}` when it has none (ADR 0003).
    ///
    /// # Errors
    ///
    /// Returns `404 event_not_found` or `409 event_not_reserved`.
    pub async fn display_state(&self, event_id: &str) -> Result<Value, ApiError> {
        let (_, event) = self.get_reserved_entry(event_id).await?;
        let state = event.display.read().await.state.clone();
        Ok(state)
    }

    /// Stores an image for a reserved event (ADR 0003).
    ///
    /// The order of checks is: the event exists, the token matches, the
    /// event is reserved, the body was read in full, the SHA-256 of the body
    /// is `sha256`, the body starts with the JPEG or PNG bytes, and the
    /// publish rate limit allows the request. The type comes from the first
    /// bytes only.
    ///
    /// An image that the event holds already gives `stored: false` and no
    /// change. An event holds at most two images. When it holds two, a new
    /// image first removes one image that is not the image of the present
    /// display state: the image of the state before it, else the earlier
    /// upload. The display lock is the only lock held during the change.
    ///
    /// # Errors
    ///
    /// Returns `404 event_not_found`, `403 invalid_token`,
    /// `409 event_not_reserved`, the error of `body`, `400 sha256_mismatch`,
    /// `400 unsupported_image`, or `429 publish_rate_limited`.
    pub async fn upload_artwork(
        &self,
        event_id: &str,
        sha256: &str,
        token: &str,
        body: Result<Bytes, ApiError>,
    ) -> Result<UploadArtworkResponse, ApiError> {
        let (stored, event) = self.get_entry(event_id).await?;
        if !stored.token_hash_matches(&hash_token(token)) {
            return Err(ApiError::new(StatusCode::FORBIDDEN, "invalid_token"));
        }
        if stored.class != EventClass::Reserved {
            return Err(ApiError::new(StatusCode::CONFLICT, "event_not_reserved"));
        }
        let body = body?;
        // The digest text is always 64 lowercase hexadecimal characters, so
        // this one comparison also checks the form of `sha256`.
        if sha256_hex(&body) != sha256 {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "sha256_mismatch"));
        }
        let mime = image_mime(&body)
            .ok_or_else(|| ApiError::new(StatusCode::BAD_REQUEST, "unsupported_image"))?;

        let now = self.now();
        if !event.try_acquire_publish_slot(now, self.inner.config.max_publishes_per_event_per_sec) {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "publish_rate_limited",
            ));
        }
        event.touch(now);

        // Copy the bytes, so the image does not keep a larger receive buffer
        // in memory.
        let image = StoredImage {
            sha256: sha256.to_string(),
            mime,
            bytes: Bytes::copy_from_slice(&body),
        };
        let mut display = event.display.write().await;
        let added = display.store_image(image);
        drop(display);

        Ok(UploadArtworkResponse {
            event_id: event_id.to_string(),
            sha256: sha256.to_string(),
            mime: mime.to_string(),
            stored: added,
        })
    }

    /// Returns the type and the bytes of an image of a reserved event
    /// (ADR 0003).
    ///
    /// # Errors
    ///
    /// Returns `404 event_not_found`, `409 event_not_reserved`, or
    /// `404 artwork_not_found` when the event does not hold the image.
    pub async fn artwork(
        &self,
        event_id: &str,
        sha256: &str,
    ) -> Result<(&'static str, Bytes), ApiError> {
        let (_, event) = self.get_reserved_entry(event_id).await?;
        let display = event.display.read().await;
        display
            .image(sha256)
            .map(|image| (image.mime, image.bytes.clone()))
            .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "artwork_not_found"))
    }

    async fn subscribe_display(
        &self,
        event_id: &str,
        last_event_id: Option<u64>,
    ) -> Result<DisplaySubscription, ApiError> {
        let previous = self.inner.sse_connections.fetch_add(1, Ordering::AcqRel);
        if previous >= self.inner.config.max_sse_connections {
            self.inner.sse_connections.fetch_sub(1, Ordering::AcqRel);
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "max_sse_connections_reached",
            ));
        }

        let event = match self.get_reserved_entry(event_id).await {
            Ok((_, event)) => event,
            Err(err) => {
                self.inner.sse_connections.fetch_sub(1, Ordering::AcqRel);
                return Err(err);
            }
        };

        event.touch(self.now());
        // Subscribe under the read lock. A publish applies its state under the
        // write lock and sends after it, so no state falls between the copy
        // and the subscription. A state that the copy holds can also arrive on
        // the receiver. The stream skips it by its `seq`.
        let (replay, subscribed_seq, receiver) = {
            let inner = event.display.read().await;
            let replay = match last_event_id {
                Some(last_event_id) => inner
                    .replay
                    .iter()
                    .filter(|update| update.seq > last_event_id)
                    .cloned()
                    .collect(),
                // A client with no `Last-Event-ID` gets the present state
                // first (ADR 0003, amended 2026-10-04).
                None => vec![DisplayUpdate {
                    seq: inner.seq,
                    state: inner.state.clone(),
                }],
            };
            (replay, inner.seq, event.display_sender.subscribe())
        };
        let closed = event.closed.subscribe();

        Ok(DisplaySubscription {
            state: self.clone(),
            event,
            replay,
            subscribed_seq,
            receiver,
            closed,
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
        let closed = event.closed.subscribe();

        Ok(EventSubscription {
            state: self.clone(),
            event,
            replay,
            receiver,
            closed,
        })
    }

    /// Removes each ephemeral event whose last activity is older than the
    /// idle TTL.
    ///
    /// The reaper never removes a reserved item (ADR 0001). It reads the
    /// class from the store under the table lock, and it takes no event lock.
    /// Because it keeps each reserved item, it never deletes a row of the
    /// state file.
    ///
    /// Returns the count of removed events.
    pub async fn cleanup_expired(&self) -> usize {
        let cutoff = self.now().saturating_sub(self.inner.config.ttl.as_secs());
        let mut events = self.inner.events.write().await;
        let EventTable { store, live } = &mut *events;
        let removed = match store.retain(&mut |stored| {
            stored.class == EventClass::Reserved
                || live
                    .get(&stored.event_id)
                    .is_some_and(|event| event.last_activity.load(Ordering::Relaxed) >= cutoff)
        }) {
            Ok(removed) => removed,
            Err(err) => {
                tracing::error!(error = %err, "cannot remove expired live items");
                return 0;
            }
        };
        for event_id in &removed {
            live.remove(event_id);
        }
        removed.len()
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
    /// A cleared lease also sets a display state that is not
    /// `{"track": null}` to `{"track": null}`, and sends it on
    /// `/display/events` (ADR 0003). The display lock is taken after the
    /// live value lock is released, and no lock is held during a send.
    ///
    /// Returns the count of events cleared this way.
    pub async fn expire_leases(&self) -> usize {
        let now = self.now();
        let lease_secs = self.inner.config.lease.as_secs();

        // Copy the candidates, then release the table lock before the first
        // event lock.
        let candidates: Vec<(String, Arc<LiveEventState>)> = {
            let events = self.inner.events.read().await;
            events
                .store
                .list_ids()
                .into_iter()
                .filter_map(|event_id| {
                    let event = Arc::clone(events.live.get(&event_id)?);
                    Some((event_id, event))
                })
                .collect()
        };

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
            if now.saturating_sub(renewed_at) < lease_secs {
                continue;
            }
            if inner.latest.is_none() {
                // No snapshot to clear. The lease still ended, so the display
                // state clears (ADR 0003). This covers an event that published
                // a display state and no payload, for example after a restart.
                drop(inner);
                self.clear_display_after_lease(&event, lease_secs, now)
                    .await;
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

            self.clear_display_after_lease(&event, lease_secs, now)
                .await;
            expired_count += 1;
        }

        expired_count
    }

    /// Sets the display state of `event` to `{"track": null}` and sends it,
    /// when its lease ended and the state is not null already. When the
    /// lease ended, it also removes every image of the event (ADR 0003),
    /// with or without a display state.
    ///
    /// The caller holds no lock. This takes only the `display` lock, checks the
    /// lease again under it, and sends after it releases the lock.
    async fn clear_display_after_lease(&self, event: &LiveEventState, lease_secs: u64, now: u64) {
        let mut display = event.display.write().await;
        let renewed_at = event.renewed_at.load(Ordering::Relaxed);
        let ended = now.saturating_sub(renewed_at) >= lease_secs;
        let cleared = (ended && !display.state["track"].is_null())
            .then(|| display.apply(null_display_state()));
        if ended {
            display.remove_images();
        }
        drop(display);
        if let Some(update) = cleared {
            let _ = event.display_sender.send(update);
        }
    }

    fn attach_socket_io(&self, io: SocketIo) {
        self.inner
            .socket_io
            .set(io)
            .expect("socket_io already attached; app(state) called twice on same RelayState");
    }

    /// Sends `{}` to each Socket.IO client of a deleted event, then
    /// disconnects it. A new client of an unknown event gets the same
    /// treatment from the namespace handler.
    async fn disconnect_socket_clients(&self, event_id: &str) {
        self.emit_socket_remote_value(event_id, &json!({})).await;
        if let Some(io) = self.inner.socket_io.get()
            && let Some(event_ns) = io.of("/event")
            && let Err(err) = event_ns.to(event_id.to_string()).disconnect().await
        {
            tracing::warn!(%event_id, ?err, "failed to disconnect Socket.IO clients");
        }
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

fn unique_event_id(store: &dyn EventStore) -> String {
    loop {
        let event_id = random_urlsafe(EVENT_ID_BYTES);
        if store.get(&event_id).is_none() {
            return event_id;
        }
    }
}

/// The live half of an event. It is never stored.
struct LiveEventState {
    last_activity: AtomicU64,
    renewed_at: AtomicU64,
    publish_window: AtomicU64,
    /// The time of the last accepted metadata publish. Only a publish sets
    /// it. A keepalive does not. It is in memory only, so a restart clears
    /// it (ADR 0001).
    last_publish_at: Mutex<Option<u64>>,
    inner: RwLock<LiveEventInner>,
    sender: broadcast::Sender<MetadataSnapshot>,
    /// Changes to `true` when an operator deletes the event. Each SSE stream
    /// of the event then stops. Only a delete sets it, and only a reserved
    /// item can be deleted.
    closed: watch::Sender<bool>,
    /// The display state of ADR 0003. Only a reserved event uses it. It is
    /// separate from `inner`, and no live value transport reads it.
    display: RwLock<DisplayInner>,
    /// The sender of the `/display/events` stream. It never carries a live
    /// value update.
    display_sender: broadcast::Sender<DisplayUpdate>,
}

struct LiveEventInner {
    seq: u64,
    latest: Option<MetadataSnapshot>,
    replay: VecDeque<MetadataSnapshot>,
}

/// The display half of an event (ADR 0003). It is in memory only, so a
/// restart gives `{"track": null}` and no image.
///
/// One lock holds the display state and the images. So the check that a
/// display state names a held image, the state change and the retention
/// rule are one step for each other writer.
struct DisplayInner {
    /// The sequence number of the display stream. It is separate from the
    /// `seq` of the live value.
    seq: u64,
    /// The present display state. It is always a validated display state.
    state: Value,
    replay: VecDeque<DisplayUpdate>,
    /// The images of the event, oldest first. At most `MAX_EVENT_IMAGES`.
    images: Vec<StoredImage>,
    /// The `artwork.sha256` of the display state before the present one.
    previous_sha256: Option<String>,
}

/// One image in the artwork store. Its type comes from its first bytes.
struct StoredImage {
    sha256: String,
    mime: &'static str,
    bytes: Bytes,
}

impl DisplayInner {
    fn new() -> Self {
        Self {
            seq: 0,
            state: null_display_state(),
            replay: VecDeque::with_capacity(REPLAY_CAPACITY),
            images: Vec::with_capacity(MAX_EVENT_IMAGES),
            previous_sha256: None,
        }
    }

    fn image(&self, sha256: &str) -> Option<&StoredImage> {
        self.images.iter().find(|image| image.sha256 == sha256)
    }

    /// Adds `image` and returns `true`, or returns `false` when the event
    /// holds it already.
    ///
    /// When the event holds `MAX_EVENT_IMAGES` images, this first removes
    /// one image that the present display state does not name. It prefers
    /// the image of the state before the present one, else the oldest
    /// image. The image of the present state stays.
    fn store_image(&mut self, image: StoredImage) -> bool {
        if self.image(&image.sha256).is_some() {
            return false;
        }
        if self.images.len() >= MAX_EVENT_IMAGES {
            let present = artwork_sha256(&self.state);
            let previous = self.previous_sha256.as_deref();
            let candidates = || {
                self.images
                    .iter()
                    .enumerate()
                    .filter(|(_, held)| Some(held.sha256.as_str()) != present)
            };
            let evict = candidates()
                .find(|(_, held)| Some(held.sha256.as_str()) == previous)
                .or_else(|| candidates().next())
                .map(|(index, _)| index);
            if let Some(index) = evict {
                self.images.remove(index);
            }
        }
        self.images.push(image);
        true
    }

    /// Publishes a validated display state.
    ///
    /// An `artwork.sha256` must name a held image, and its `mime` must be
    /// the stored type. After the change, the event keeps only the image of
    /// the present state and the image of the state before it.
    ///
    /// # Errors
    ///
    /// Returns `409 artwork_missing` or `400 invalid_display`. An error
    /// changes nothing.
    fn publish(&mut self, state: Value) -> Result<DisplayUpdate, ApiError> {
        if let Some(sha256) = artwork_sha256(&state) {
            let image = self
                .image(sha256)
                .ok_or_else(|| ApiError::new(StatusCode::CONFLICT, "artwork_missing"))?;
            if state["track"]["artwork"]["mime"].as_str() != Some(image.mime) {
                return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid_display"));
            }
        }
        self.previous_sha256 = artwork_sha256(&self.state).map(str::to_string);
        let update = self.apply(state);
        let present = artwork_sha256(&self.state);
        let previous = self.previous_sha256.as_deref();
        self.images.retain(|image| {
            let held = Some(image.sha256.as_str());
            held == present || held == previous
        });
        Ok(update)
    }

    /// Removes every image. A lease end calls this (ADR 0003).
    fn remove_images(&mut self) {
        self.images.clear();
        self.previous_sha256 = None;
    }

    /// Sets the present state, advances `seq`, and adds the update to the
    /// replay buffer. The caller sends the returned update after it releases
    /// the lock.
    fn apply(&mut self, state: Value) -> DisplayUpdate {
        self.seq += 1;
        self.state = state;
        let update = DisplayUpdate {
            seq: self.seq,
            state: self.state.clone(),
        };
        self.replay.push_back(update.clone());
        while self.replay.len() > REPLAY_CAPACITY {
            self.replay.pop_front();
        }
        update
    }
}

/// One update of the display stream.
#[derive(Debug, Clone)]
struct DisplayUpdate {
    seq: u64,
    state: Value,
}

/// The display state when nothing plays.
fn null_display_state() -> Value {
    json!({ "track": null })
}

/// Returns the `artwork.sha256` of a validated display state, if it has one.
fn artwork_sha256(state: &Value) -> Option<&str> {
    state["track"]["artwork"]["sha256"].as_str()
}

/// Returns the SHA-256 of `bytes` as 64 lowercase hexadecimal characters.
fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// Returns the image type from the first bytes of `body`, or `None` when
/// the body is not a JPEG or a PNG image.
fn image_mime(body: &[u8]) -> Option<&'static str> {
    if body.starts_with(JPEG_MAGIC) {
        Some("image/jpeg")
    } else if body.starts_with(PNG_MAGIC) {
        Some("image/png")
    } else {
        None
    }
}

impl LiveEventState {
    fn new(now: u64) -> Self {
        let (sender, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            last_activity: AtomicU64::new(now),
            renewed_at: AtomicU64::new(now),
            publish_window: AtomicU64::new(0),
            last_publish_at: Mutex::new(None),
            inner: RwLock::new(LiveEventInner {
                seq: 0,
                latest: None,
                replay: VecDeque::with_capacity(REPLAY_CAPACITY),
            }),
            sender,
            closed: watch::channel(false).0,
            display: RwLock::new(DisplayInner::new()),
            display_sender: broadcast::channel(BROADCAST_CAPACITY).0,
        }
    }

    fn try_acquire_publish_slot(&self, now: u64, limit: u32) -> bool {
        try_acquire_window_slot(&self.publish_window, now, limit)
    }

    fn touch(&self, now: u64) {
        self.last_activity.store(now, Ordering::Relaxed);
    }

    fn renew_lease(&self, now: u64) {
        self.renewed_at.store(now, Ordering::Relaxed);
    }

    fn record_publish(&self, now: u64) {
        *self
            .last_publish_at
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(now);
    }

    fn last_publish_at(&self) -> Option<u64> {
        *self
            .last_publish_at
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Stops each SSE stream of the event. A stream that starts later stops
    /// at once.
    fn close(&self) {
        self.closed.send_replace(true);
    }
}

struct EventSubscription {
    state: RelayState,
    event: Arc<LiveEventState>,
    replay: Vec<MetadataSnapshot>,
    receiver: broadcast::Receiver<MetadataSnapshot>,
    closed: watch::Receiver<bool>,
}

impl Drop for EventSubscription {
    fn drop(&mut self) {
        self.state
            .inner
            .sse_connections
            .fetch_sub(1, Ordering::AcqRel);
    }
}

/// A subscriber of `/display/events`. It counts against the same SSE
/// connection limit as `/events`.
struct DisplaySubscription {
    state: RelayState,
    event: Arc<LiveEventState>,
    replay: Vec<DisplayUpdate>,
    /// The display `seq` when the receiver subscribed. A received update with
    /// this `seq` or a lower one was applied before the subscription, so the
    /// replay or the client holds it already.
    subscribed_seq: u64,
    receiver: broadcast::Receiver<DisplayUpdate>,
    closed: watch::Receiver<bool>,
}

impl Drop for DisplaySubscription {
    fn drop(&mut self) {
        self.state
            .inner
            .sse_connections
            .fetch_sub(1, Ordering::AcqRel);
    }
}

/// Validates a display state body (ADR 0003 §The Display State).
///
/// The body is an object with the one key `track`. `track` is `null`, or an
/// object with exactly the keys `artist`, `title` and `artwork`. `artist` and
/// `title` are strings. `artwork` is `null`, `{"sha256", "mime"}` or
/// `{"url"}`. Returns the body as the relay stores it.
///
/// # Errors
///
/// Returns `400 invalid_display` for each other shape.
fn validate_display(body: &[u8]) -> Result<Value, ApiError> {
    let invalid = || ApiError::new(StatusCode::BAD_REQUEST, "invalid_display");
    let state: Value = serde_json::from_slice(body).map_err(|_| invalid())?;

    let root = state.as_object().ok_or_else(invalid)?;
    if root.len() != 1 {
        return Err(invalid());
    }
    let track = root.get("track").ok_or_else(invalid)?;
    if track.is_null() {
        return Ok(state);
    }

    let track = track.as_object().ok_or_else(invalid)?;
    if track.len() != 3 || !track.get("artist").is_some_and(Value::is_string) {
        return Err(invalid());
    }
    if !track.get("title").is_some_and(Value::is_string) {
        return Err(invalid());
    }
    let artwork = track.get("artwork").ok_or_else(invalid)?;
    if artwork.is_null() {
        return Ok(state);
    }

    let artwork = artwork.as_object().ok_or_else(invalid)?;
    let valid = match artwork.len() {
        1 => artwork
            .get("url")
            .and_then(Value::as_str)
            .is_some_and(is_valid_artwork_url),
        2 => {
            artwork
                .get("sha256")
                .and_then(Value::as_str)
                .is_some_and(is_sha256_hex)
                && artwork
                    .get("mime")
                    .and_then(Value::as_str)
                    .is_some_and(|mime| ARTWORK_MIME_TYPES.contains(&mime))
        }
        _ => false,
    };
    if !valid {
        return Err(invalid());
    }
    Ok(state)
}

/// Returns `true` for an `http` or `https` URL of at most
/// `MAX_ARTWORK_URL_CHARS` characters with a part after the scheme.
fn is_valid_artwork_url(url: &str) -> bool {
    if url.chars().count() > MAX_ARTWORK_URL_CHARS {
        return false;
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        && !rest.is_empty()
}

/// Returns `true` for exactly 64 lowercase hexadecimal characters.
fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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

/// The response of `POST /v1/liveitems/reserved`.
///
/// It holds the fields of [`CreateEventResponse`] and the label. `v4vmm`
/// parses these names. Do not rename them.
#[derive(Debug, Serialize, Deserialize)]
pub struct ReserveEventResponse {
    pub event_id: String,
    pub broadcaster_token: String,
    pub metadata_url: String,
    pub remote_value_url: String,
    pub events_url: String,
    pub socket_io_url: String,
    /// The operator name. It is absent when the request holds no label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// The response of `GET /v1/liveitems/reserved`.
///
/// It is an object, not a bare array, so a later field does not change the
/// shape that `v4vmm` parses. An empty list keeps the `reserved` key.
#[derive(Debug, Serialize, Deserialize)]
pub struct ReservedItemsResponse {
    /// The reserved items, oldest first.
    pub reserved: Vec<ReservedItem>,
}

/// One reserved item in the list. It holds no token and no token hash.
#[derive(Debug, Serialize, Deserialize)]
pub struct ReservedItem {
    /// The event identifier.
    pub event_id: String,
    /// The operator name. It is absent when the item has no label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The creation time, in RFC 3339.
    pub created_at: String,
    /// The time of the last accepted publish, in RFC 3339. It is absent
    /// when the item has not published since the relay started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_publish_at: Option<String>,
}

/// The body of `POST /v1/liveitems/reserved`. Each field is optional.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReserveEventRequest {
    #[serde(default)]
    label: Option<String>,
}

impl ReserveEventRequest {
    /// Parses the body. An empty body is a request with no label.
    fn parse(body: &[u8]) -> Result<Self, ApiError> {
        if body.iter().all(u8::is_ascii_whitespace) {
            return Ok(Self::default());
        }
        let request: Self = serde_json::from_slice(body)
            .map_err(|_| ApiError::new(StatusCode::BAD_REQUEST, "invalid_body"))?;
        if let Some(label) = &request.label
            && (label.trim().is_empty() || label.chars().count() > MAX_LABEL_CHARS)
        {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "invalid_label"));
        }
        Ok(request)
    }
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

/// The response of `POST /v1/liveitems/{event_id}/display`.
#[derive(Debug, Serialize, Deserialize)]
pub struct PublishDisplayResponse {
    pub event_id: String,
    pub accepted: bool,
    /// The sequence number of the display stream. It is separate from the
    /// `seq` of the live value.
    pub seq: u64,
}

/// The response of `PUT /v1/liveitems/{event_id}/artwork/{sha256}`.
#[derive(Debug, Serialize, Deserialize)]
pub struct UploadArtworkResponse {
    pub event_id: String,
    pub sha256: String,
    /// The image type that the relay read from the first bytes.
    pub mime: String,
    /// `true` when this request stored the image. `false` when the event
    /// held it already, and nothing changed.
    pub stored: bool,
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

impl From<StoreError> for ApiError {
    /// Maps a store error to a stable error code. The body holds no internal
    /// detail, so the cause goes to the log only.
    fn from(err: StoreError) -> Self {
        match err {
            StoreError::LabelTaken => Self::new(StatusCode::CONFLICT, "label_already_reserved"),
            StoreError::ReservedLimitReached => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "max_reserved_items_reached",
            ),
            err => {
                tracing::error!(error = %err, "event store failed");
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "store_unavailable")
            }
        }
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
    let artwork_max_bytes = state.inner.config.artwork_max_bytes;
    let (socket_layer, io) = SocketIo::builder().with_state(state.clone()).build_layer();

    register_socket_namespaces(io.clone());
    state.attach_socket_io(io);

    Router::new()
        .route("/health", get(health))
        .route("/v1/liveitems/health", get(health))
        .route("/v1/liveitems", post(create_event))
        .route("/v1/liveitems/", post(create_event))
        .route(
            "/v1/liveitems/reserved",
            post(reserve_event)
                .get(list_reserved)
                .route_layer(RequestBodyLimitLayer::new(RESERVE_BODY_LIMIT_BYTES)),
        )
        .route("/v1/liveitems/reserved/{event_id}", delete(delete_reserved))
        .route(
            "/v1/liveitems/{event_id}/metadata",
            get(latest_metadata)
                .post(publish_metadata)
                .route_layer(RequestBodyLimitLayer::new(METADATA_BODY_LIMIT_BYTES)),
        )
        .route("/v1/liveitems/{event_id}/remoteValue", get(remote_value))
        .route("/v1/liveitems/{event_id}/events", get(events))
        .route("/v1/liveitems/{event_id}/keepalive", post(keepalive_route))
        .route(
            "/v1/liveitems/{event_id}/display",
            get(display_state)
                .post(publish_display)
                .route_layer(RequestBodyLimitLayer::new(DISPLAY_BODY_LIMIT_BYTES)),
        )
        .route(
            "/v1/liveitems/{event_id}/display/events",
            get(display_events),
        )
        .route(
            "/v1/liveitems/{event_id}/artwork/{sha256}",
            get(read_artwork)
                .put(upload_artwork)
                .route_layer(RequestBodyLimitLayer::new(artwork_max_bytes)),
        )
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

/// Handles `POST /v1/liveitems/reserved`.
///
/// The handler checks the admin credential before it reads the body, so a
/// request with no credential never gets a body error.
async fn reserve_event(
    AxumState(state): AxumState<RelayState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<ReserveEventResponse>), ApiError> {
    state.authorize_admin(&headers)?;
    let request = ReserveEventRequest::parse(&body)?;
    let reserved = state.reserve_event(request.label).await?;
    Ok((StatusCode::CREATED, Json(reserved)))
}

/// Handles `GET /v1/liveitems/reserved`.
async fn list_reserved(
    AxumState(state): AxumState<RelayState>,
    headers: HeaderMap,
) -> Result<Json<ReservedItemsResponse>, ApiError> {
    state.authorize_admin(&headers)?;
    Ok(Json(state.list_reserved().await))
}

/// Handles `DELETE /v1/liveitems/reserved/{event_id}`.
///
/// The handler checks the admin credential before it looks for the item.
async fn delete_reserved(
    AxumState(state): AxumState<RelayState>,
    Path(event_id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    state.authorize_admin(&headers)?;
    state.delete_reserved(&event_id).await?;
    Ok(StatusCode::NO_CONTENT)
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

/// Handles `POST /v1/liveitems/{event_id}/display` (ADR 0003).
///
/// The relay checks the credential, the event, and the class before the
/// result of the body read. A body over the limit then gives
/// `413 payload_too_large`. A request with a `Content-Length` over the limit
/// gets `413` from `RequestBodyLimitLayer` before this handler runs.
async fn publish_display(
    AxumState(state): AxumState<RelayState>,
    Path(event_id): Path<String>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<PublishDisplayResponse>, ApiError> {
    let token = bearer_token(&headers)?;
    let body = body.map_err(|rejection| {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large")
        } else {
            ApiError::new(StatusCode::BAD_REQUEST, "invalid_display")
        }
    });
    Ok(Json(state.publish_display(&event_id, token, body).await?))
}

/// Handles `GET /v1/liveitems/{event_id}/display` (ADR 0003).
async fn display_state(
    AxumState(state): AxumState<RelayState>,
    Path(event_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(state.display_state(&event_id).await?))
}

/// Handles `PUT /v1/liveitems/{event_id}/artwork/{sha256}` (ADR 0003).
///
/// The relay checks the credential, the event, and the class before the
/// result of the body read. A streamed body over `ARTWORK_MAX_BYTES` then
/// gives `413 payload_too_large`. A request with a `Content-Length` over the
/// limit gets `413` from `RequestBodyLimitLayer` before this handler runs.
async fn upload_artwork(
    AxumState(state): AxumState<RelayState>,
    Path((event_id, sha256)): Path<(String, String)>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<UploadArtworkResponse>, ApiError> {
    let token = bearer_token(&headers)?;
    let body = body.map_err(|rejection| {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large")
        } else {
            ApiError::new(StatusCode::BAD_REQUEST, "unsupported_image")
        }
    });
    Ok(Json(
        state
            .upload_artwork(&event_id, &sha256, token, body)
            .await?,
    ))
}

/// Handles `GET /v1/liveitems/{event_id}/artwork/{sha256}` (ADR 0003).
///
/// It gives the bytes with the stored type, a cache time of one year, and
/// `X-Content-Type-Options: nosniff`.
async fn read_artwork(
    AxumState(state): AxumState<RelayState>,
    Path((event_id, sha256)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let (mime, bytes) = state.artwork(&event_id, &sha256).await?;
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, ARTWORK_CACHE_CONTROL),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    )
        .into_response())
}

/// Handles `GET /v1/liveitems/{event_id}/display/events` (ADR 0003).
///
/// It is an SSE stream of `display` events with its own sequence number and
/// its own replay buffer. It never carries a `remoteValue` event, and
/// `/events` never carries a `display` event.
async fn display_events(
    AxumState(state): AxumState<RelayState>,
    Path(event_id): Path<String>,
    headers: HeaderMap,
) -> Result<Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let last_event_id = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());

    let subscription = state.subscribe_display(&event_id, last_event_id).await?;

    let stream = async_stream::stream! {
        let mut subscription = subscription;
        // A received state with a `seq` up to the one at the subscription was
        // applied before it. The replay or the client holds it, so the stream
        // skips it. Do not start from `Last-Event-ID`: after a relay restart the
        // `seq` starts again at 1, and a client can send a higher old value.
        let mut sent_seq = Some(subscription.subscribed_seq);

        for update in subscription.replay.drain(..) {
            yield Ok(display_update_to_sse(update));
        }

        loop {
            // A delete closes the event, as for `/events`.
            let received = tokio::select! {
                biased;
                _ = subscription.closed.wait_for(|closed| *closed) => break,
                received = subscription.receiver.recv() => received,
            };
            match received {
                Ok(update) => {
                    if sent_seq.is_some_and(|sent| update.seq <= sent) {
                        continue;
                    }
                    subscription.event.touch(subscription.state.now());
                    sent_seq = Some(update.seq);
                    yield Ok(display_update_to_sse(update));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

fn display_update_to_sse(update: DisplayUpdate) -> Event {
    Event::default()
        .event("display")
        .id(update.seq.to_string())
        .json_data(update.state)
        .expect("display states are serializable")
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
            // A delete closes the event. The stream then ends, and a
            // reconnect gets `404`. The flag never changes for an ephemeral
            // event, because an ephemeral event cannot be deleted.
            let received = tokio::select! {
                biased;
                _ = subscription.closed.wait_for(|closed| *closed) => break,
                received = subscription.receiver.recv() => received,
            };
            match received {
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
                ..AppConfig::for_tests()
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

    #[test]
    fn artwork_max_bytes_has_a_default_and_reads_the_environment() {
        assert_eq!(DEFAULT_ARTWORK_MAX_BYTES, 524_288);
        assert_eq!(
            AppConfig::for_tests().artwork_max_bytes,
            DEFAULT_ARTWORK_MAX_BYTES
        );
        // SAFETY: this test is the only one in the suite that reads or
        // writes ARTWORK_MAX_BYTES. It calls only the parser of that one
        // variable, so it does not race the LEASE_SECS test.
        unsafe {
            std::env::set_var("ARTWORK_MAX_BYTES", "1000");
        }
        let set = parse_artwork_max_bytes();
        // SAFETY: see above.
        unsafe {
            std::env::remove_var("ARTWORK_MAX_BYTES");
        }
        assert_eq!(set.expect("a valid value"), 1000);
        assert_eq!(
            parse_artwork_max_bytes().expect("the default"),
            DEFAULT_ARTWORK_MAX_BYTES
        );
    }

    #[test]
    fn admin_token_matches_only_the_same_token() {
        let admin_token = AdminToken::new("test-admin-token-0123456789");

        assert!(admin_token.matches("test-admin-token-0123456789"));
        assert!(!admin_token.matches("test-admin-token-0123456788"));
        assert!(!admin_token.matches("test-admin-token-012345678"));
        assert!(!admin_token.matches(""));
    }

    /// Returns the production part of a source file: the text before its
    /// test module.
    fn production_source(source: &'static str) -> &'static str {
        source
            .split_once("#[cfg(test)]")
            .map_or(source, |(production, _)| production)
    }

    /// Returns the text of the function that starts at `signature`, up to
    /// the first line that closes a block at the indent of the signature.
    fn function_body(source: &'static str, signature: &str) -> &'static str {
        let start = source
            .find(signature)
            .unwrap_or_else(|| panic!("the source has no `{signature}`"));
        let indent = source[..start]
            .rsplit_once('\n')
            .map_or(start, |(_, line)| line.len());
        let close = format!("\n{}}}\n", " ".repeat(indent));
        let end = source[start..]
            .find(&close)
            .unwrap_or_else(|| panic!("`{signature}` has no end"));
        &source[start..start + end]
    }

    // ADR 0001 §Invariants: token comparison stays constant time, for the
    // admin token and for the broadcaster token. A test cannot measure the
    // time of a comparison reliably. This guard proves that both checks go
    // through `subtle::ConstantTimeEq` and that no hash is compared with
    // `==` or `!=`.
    #[test]
    fn both_token_checks_use_the_constant_time_comparison() {
        const FIX: &str = "ADR 0001 §Invariants and AGENTS.md §4: compare a token hash with \
                           `subtle::ConstantTimeEq::ct_eq`, never with `==`. Route each token \
                           check through `AdminToken::matches` or \
                           `StoredEvent::token_hash_matches`.";
        let lib = production_source(include_str!("lib.rs"));
        let store = production_source(include_str!("store.rs"));

        assert!(
            function_body(lib, "fn matches(&self, candidate: &str)").contains(".ct_eq("),
            "{FIX} `AdminToken::matches` does not call `ct_eq`."
        );
        assert!(
            function_body(store, "pub fn token_hash_matches(").contains(".ct_eq("),
            "{FIX} `StoredEvent::token_hash_matches` does not call `ct_eq`."
        );

        // The admin check, the publish check, and the keepalive check each
        // call one of the two functions.
        assert!(
            function_body(lib, "fn authorize_admin(").contains("admin_token.matches("),
            "{FIX} The admin check does not call `AdminToken::matches`."
        );
        for signature in ["pub async fn publish_metadata(", "pub async fn keepalive("] {
            assert!(
                function_body(lib, signature).contains(".token_hash_matches(&hash_token(token))"),
                "{FIX} `{signature}` does not call `StoredEvent::token_hash_matches`."
            );
        }

        for (file, source) in [("src/lib.rs", lib), ("src/store.rs", store)] {
            for (index, line) in source.lines().enumerate() {
                let code = line.split_once("//").map_or(line, |(code, _)| code);
                let compares = code.contains("==") || code.contains("!=");
                let names_a_secret = code.contains("hash") || code.contains("token");
                assert!(
                    !(compares && names_a_secret),
                    "{FIX} {file}:{} compares a token or a hash with `==` or `!=`: {line}",
                    index + 1
                );
            }
        }
    }

    // ADR 0003 §Routes: a display publish checks the broadcaster token as a
    // publish does. This guard proves that the check goes through
    // `StoredEvent::token_hash_matches`, which calls `ct_eq`.
    #[test]
    fn the_display_token_check_uses_the_constant_time_comparison() {
        let lib = production_source(include_str!("lib.rs"));
        assert!(
            function_body(lib, "pub async fn publish_display(")
                .contains(".token_hash_matches(&hash_token(token))"),
            "ADR 0003 §Routes and AGENTS.md §4: `publish_display` must check the token with \
             `StoredEvent::token_hash_matches`, never with `==`."
        );
    }

    // ADR 0003 §Routes: an image upload checks the broadcaster token as a
    // publish does. This guard proves that the check goes through
    // `StoredEvent::token_hash_matches`, which calls `ct_eq`.
    #[test]
    fn the_artwork_token_check_uses_the_constant_time_comparison() {
        let lib = production_source(include_str!("lib.rs"));
        assert!(
            function_body(lib, "pub async fn upload_artwork(")
                .contains(".token_hash_matches(&hash_token(token))"),
            "ADR 0003 §Routes and AGENTS.md §4: `upload_artwork` must check the token with \
             `StoredEvent::token_hash_matches`, never with `==`."
        );
    }
}
