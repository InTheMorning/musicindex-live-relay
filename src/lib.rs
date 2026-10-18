pub mod store;

use std::{
    collections::{HashMap, VecDeque},
    convert::Infallible,
    fmt::Write as _,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::{
        Arc, Mutex, Once, OnceLock, PoisonError,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{ConnectInfo, FromRequestParts, Path, State as AxumState, rejection::BytesRejection},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
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
/// The maximum length of a display track `songLine`, in characters (ADR 0005).
const MAX_SONG_LINE_CHARS: usize = 1_024;
/// The maximum length of a display track `eventGuid` or `blockGuid`, in
/// characters (ADR 0005).
const MAX_VALUE_GUID_CHARS: usize = 128;
/// The maximum length of a display track `album`, in characters (ADR 0006).
const MAX_ALBUM_CHARS: usize = 1_024;
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
/// The default of `MAX_LISTENER_DELAY_SECS`, the highest delay that a
/// publish can set (ADR 0004 §Configuration).
const DEFAULT_MAX_LISTENER_DELAY_SECS: u64 = 300;
/// The default of `MAX_PENDING_LISTENER_UPDATES`, the pending listener
/// updates that the relay keeps for each event (ADR 0004 §Configuration).
const DEFAULT_MAX_PENDING_LISTENER_UPDATES: usize = 64;
/// The lowest accepted value of `MAX_PENDING_LISTENER_UPDATES`. A bound of
/// 0 would leave no slot for a delayed update, so every one of them would
/// have nowhere to wait.
const MIN_PENDING_LISTENER_UPDATES: usize = 1;
/// The lowercase name of the listener delay header (ADR 0004 §The Delay).
/// `HeaderMap` looks up a header by its lowercase name, so the broadcaster
/// can send the header in any case.
const LISTENER_DELAY_HEADER: &str = "listener-delay-secs";
/// The default of `TRUSTED_PROXIES` (ADR 0007 §The Client Address). The
/// default `BIND` sits behind a local proxy, so the default trusts
/// loopback only.
const DEFAULT_TRUSTED_PROXIES: &str = "127.0.0.1,::1";
/// The default of `MAX_LISTENER_CONNECTIONS_PER_CLIENT`, the open listener
/// connections of one client address (ADR 0007 §Three Limits).
const DEFAULT_MAX_LISTENER_CONNECTIONS_PER_CLIENT: usize = 50;
/// The default of `MAX_LISTENER_CONNECTS_PER_CLIENT_PER_SEC`, the new
/// listener connections of one client address in one second (ADR 0007
/// §Three Limits).
const DEFAULT_MAX_LISTENER_CONNECTS_PER_CLIENT_PER_SEC: u32 = 10;
/// The lowercase name of the trusted proxy's client address header (ADR
/// 0007 §The Client Address).
const X_REAL_IP_HEADER: &str = "x-real-ip";
/// The client address the relay uses when a request carries no
/// `ConnectInfo` (ADR 0007 §The Client Address). A test that gives no peer
/// address keeps working this way.
const NO_CONNECT_INFO_ADDR: IpAddr = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0));
/// The lowest length at which the client-limiter map of ADR 0007 runs its
/// amortized sweep (ADR 0007 §Three Limits, AGENTS.md §5). Not an
/// environment setting: an operator has no reason to tune it, since the
/// map is already bounded by the addresses with an open connection or a
/// recent connect, not by this value.
const CLIENT_LIMITS_SWEEP_FLOOR: usize = 1024;
/// The default of `MAX_SOCKETIO_CONNECTIONS`, the open Socket.IO
/// connections of the full relay (ADR 0007 §Three Limits).
const DEFAULT_MAX_SOCKETIO_CONNECTIONS: usize = 20_000;
/// The amount of time an Engine.IO session can stay open with no
/// namespace connect, before the relay closes it (ADR 0007 §A Socket.IO
/// Session With No Namespace).
const SOCKETIO_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The Engine.IO request path, the default of `SocketIoBuilder::req_path`
/// (ADR 0007). A request to this path with no `sid` query parameter is a
/// new Engine.IO handshake. See [`is_socketio_handshake`].
const SOCKET_IO_REQ_PATH: &str = "/socket.io";

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
    /// The highest delay that a publish can set with the
    /// `Listener-Delay-Secs` header, in seconds (ADR 0004).
    pub max_listener_delay_secs: u64,
    /// The pending listener updates that the relay keeps for each event
    /// (ADR 0004).
    pub max_pending_listener_updates: usize,
    /// The addresses and the ranges the relay trusts to set `X-Real-IP`
    /// (ADR 0007 §The Client Address, amended 2026-10-18).
    pub trusted_proxies: Vec<TrustedProxy>,
    /// The highest count of open listener connections from one client
    /// address, Socket.IO and SSE together (ADR 0007 §Three Limits). A
    /// value of 0 turns this limit off.
    pub max_listener_connections_per_client: usize,
    /// The highest count of new listener connections from one client
    /// address in one second (ADR 0007 §Three Limits). A value of 0 turns
    /// this limit off.
    pub max_listener_connects_per_client_per_sec: u32,
    /// The highest count of open Socket.IO connections of the full relay
    /// (ADR 0007 §Three Limits).
    pub max_socketio_connections: usize,
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
            max_listener_delay_secs: env_parse(
                "MAX_LISTENER_DELAY_SECS",
                DEFAULT_MAX_LISTENER_DELAY_SECS,
            )?,
            max_pending_listener_updates: parse_max_pending_listener_updates()?,
            trusted_proxies: parse_trusted_proxies()?,
            max_listener_connections_per_client: env_parse(
                "MAX_LISTENER_CONNECTIONS_PER_CLIENT",
                DEFAULT_MAX_LISTENER_CONNECTIONS_PER_CLIENT,
            )?,
            max_listener_connects_per_client_per_sec: env_parse(
                "MAX_LISTENER_CONNECTS_PER_CLIENT_PER_SEC",
                DEFAULT_MAX_LISTENER_CONNECTS_PER_CLIENT_PER_SEC,
            )?,
            max_socketio_connections: env_parse(
                "MAX_SOCKETIO_CONNECTIONS",
                DEFAULT_MAX_SOCKETIO_CONNECTIONS,
            )?,
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
            max_listener_delay_secs: DEFAULT_MAX_LISTENER_DELAY_SECS,
            max_pending_listener_updates: DEFAULT_MAX_PENDING_LISTENER_UPDATES,
            trusted_proxies: default_trusted_proxies(),
            max_listener_connections_per_client: DEFAULT_MAX_LISTENER_CONNECTIONS_PER_CLIENT,
            max_listener_connects_per_client_per_sec:
                DEFAULT_MAX_LISTENER_CONNECTS_PER_CLIENT_PER_SEC,
            max_socketio_connections: DEFAULT_MAX_SOCKETIO_CONNECTIONS,
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

/// Reads `MAX_PENDING_LISTENER_UPDATES`, the pending listener updates that
/// the relay keeps for each event (ADR 0004 §Configuration).
fn parse_max_pending_listener_updates() -> Result<usize, ConfigError> {
    let count = env_parse(
        "MAX_PENDING_LISTENER_UPDATES",
        DEFAULT_MAX_PENDING_LISTENER_UPDATES,
    )?;
    if count < MIN_PENDING_LISTENER_UPDATES {
        return Err(ConfigError {
            key: "MAX_PENDING_LISTENER_UPDATES",
            value: count.to_string(),
            message: format!("must be {MIN_PENDING_LISTENER_UPDATES} or more"),
        });
    }
    Ok(count)
}

/// One entry of `TRUSTED_PROXIES`: an address, or a CIDR range such as
/// `172.16.0.0/12` (ADR 0007 §The Client Address, amended 2026-10-18).
///
/// An entry with no `/` in `TRUSTED_PROXIES` is an address with the full
/// prefix length: 32 for an IPv4 network, 128 for an IPv6 network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrustedProxy {
    /// The network address of the entry.
    pub network: IpAddr,
    /// The prefix length of the entry, in bits.
    pub prefix_len: u8,
}

impl TrustedProxy {
    /// True when `addr` is in this entry's range.
    ///
    /// An IPv4 entry never holds an IPv6 address, and an IPv6 entry never
    /// holds an IPv4 address. Pass `addr` in canonical form
    /// ([`IpAddr::to_canonical`]), so an IPv4-mapped IPv6 address matches an
    /// IPv4 entry.
    fn contains(&self, addr: IpAddr) -> bool {
        match (self.network, addr) {
            (IpAddr::V4(network), IpAddr::V4(addr)) => {
                (u32::from(network) ^ u32::from(addr)) & ipv4_prefix_mask(self.prefix_len) == 0
            }
            (IpAddr::V6(network), IpAddr::V6(addr)) => {
                (u128::from(network) ^ u128::from(addr)) & ipv6_prefix_mask(self.prefix_len) == 0
            }
            _ => false,
        }
    }

    /// True when no bit of `self.network` sits after `self.prefix_len`.
    ///
    /// `172.18.0.1/16` fails this check, because `0.0.0.1` sits after the
    /// first 16 bits. `172.16.0.0/12` passes it.
    fn has_no_bits_after_prefix(&self) -> bool {
        match self.network {
            IpAddr::V4(network) => u32::from(network) & !ipv4_prefix_mask(self.prefix_len) == 0,
            IpAddr::V6(network) => u128::from(network) & !ipv6_prefix_mask(self.prefix_len) == 0,
        }
    }
}

impl std::fmt::Display for TrustedProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.network, self.prefix_len)
    }
}

/// The IPv4 network mask of a prefix length: the top `prefix_len` bits set,
/// the rest clear.
fn ipv4_prefix_mask(prefix_len: u8) -> u32 {
    if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix_len))
    }
}

/// The IPv6 network mask of a prefix length: the top `prefix_len` bits set,
/// the rest clear.
fn ipv6_prefix_mask(prefix_len: u8) -> u128 {
    if prefix_len == 0 {
        0
    } else {
        u128::MAX << (128 - u32::from(prefix_len))
    }
}

/// Parses one entry of `TRUSTED_PROXIES`: an address, or a CIDR range.
///
/// # Errors
///
/// Returns an error message for an address or a prefix length that does not
/// parse, for a prefix length above 32 for an IPv4 network or above 128 for
/// an IPv6 network, or for a range with a bit set after its prefix.
fn parse_trusted_proxy(entry: &str) -> Result<TrustedProxy, String> {
    let (network_text, prefix_text) = match entry.split_once('/') {
        Some((network, prefix)) => (network, Some(prefix)),
        None => (entry, None),
    };
    let network: IpAddr = network_text
        .parse()
        .map_err(|err: std::net::AddrParseError| {
            format!("{network_text:?} is not an IP address: {err}")
        })?;
    let max_prefix_len: u8 = match network {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    };
    let prefix_len = match prefix_text {
        Some(text) => {
            let prefix_len: u8 = text
                .parse()
                .map_err(|_| format!("{text:?} is not a prefix length"))?;
            if prefix_len > max_prefix_len {
                return Err(format!(
                    "the prefix length {prefix_len} is above {max_prefix_len}"
                ));
            }
            prefix_len
        }
        None => max_prefix_len,
    };
    let proxy = TrustedProxy {
        network,
        prefix_len,
    };
    if !proxy.has_no_bits_after_prefix() {
        return Err(format!("{entry:?} has a bit set after its prefix length"));
    }
    Ok(proxy)
}

/// Reads `TRUSTED_PROXIES`, the addresses and the ranges the relay trusts to
/// set `X-Real-IP` (ADR 0007 §The Client Address, amended 2026-10-18).
///
/// # Errors
///
/// Returns a `ConfigError` for a value that is not valid unicode, or that
/// holds an entry [`parse_trusted_proxy`] rejects.
fn parse_trusted_proxies() -> Result<Vec<TrustedProxy>, ConfigError> {
    let value = match std::env::var("TRUSTED_PROXIES") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(default_trusted_proxies()),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(ConfigError {
                key: "TRUSTED_PROXIES",
                value: "<not unicode>".to_string(),
                message: "not valid unicode".to_string(),
            });
        }
    };
    value
        .split(',')
        .map(|part| {
            parse_trusted_proxy(part.trim()).map_err(|message| ConfigError {
                key: "TRUSTED_PROXIES",
                value: value.clone(),
                message,
            })
        })
        .collect()
}

/// The parsed value of `DEFAULT_TRUSTED_PROXIES`.
fn default_trusted_proxies() -> Vec<TrustedProxy> {
    DEFAULT_TRUSTED_PROXIES
        .split(',')
        .map(|part| parse_trusted_proxy(part).expect("default trusted proxies are valid"))
        .collect()
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
    /// The open Socket.IO connections of the full relay (ADR 0007 §Three
    /// Limits). [`RelayState::try_acquire_socketio_slot`] and
    /// [`RelayState::release_socketio_slot`] keep it. It is the Socket.IO
    /// counterpart of `sse_connections`, checked before the client-address
    /// limiter in the `/event` namespace handler.
    socketio_connections: AtomicUsize,
    /// The open and new listener connections of each client address (ADR
    /// 0007), amortized-swept on growth. The map is bounded by the
    /// addresses that have an open connection, plus the addresses that
    /// connected in the current second: [`RelayState::client_counter`]
    /// sweeps it once its length reaches `ClientLimits::sweep_at`, and the
    /// hourly TTL reaper ([`RelayState::sweep_client_limits`]) is a second
    /// safety net.
    client_limits: Mutex<ClientLimits>,
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
                socketio_connections: AtomicUsize::new(0),
                client_limits: Mutex::new(ClientLimits::new()),
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
    /// 3. The item's `listener_order` lock is taken first (ADR 0004,
    ///    payment safety). The lock order is `listener_order`, then
    ///    `inner`. Under it, the pending listener updates leave memory.
    ///    Each SSE stream of the item stops. Each Socket.IO client of the
    ///    item gets `{}` and is disconnected. Holding `listener_order`
    ///    through the disconnect blocks a release sweep, a publish, or a
    ///    lease expiry that already copied this item. None of them can
    ///    emit a stale value for it after this step.
    ///
    /// If step 1 fails, nothing changes. Step 3 holds only
    /// `listener_order`, and briefly `inner` too.
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

        match live {
            Some(live) => {
                // Lock order (ADR 0004, payment safety): `listener_order`,
                // then `inner`. Holding `listener_order` through the
                // disconnect blocks a concurrent publish, lease expiry, or
                // release sweep of this event. None of them can emit a
                // stale listener value once the event is gone.
                let listener_order = live.listener_order.lock().await;
                // Remove the pending listener updates before the disconnect
                // (ADR 0004 §The Pending Updates), so no later release sweep
                // sends one for this event.
                live.inner.write().await.pending_listener_updates.clear();
                live.close();
                self.disconnect_socket_clients(event_id).await;
                drop(listener_order);
            }
            None => self.disconnect_socket_clients(event_id).await,
        }
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

    /// Publishes a live value update.
    ///
    /// The order of checks is: the event exists, the token matches, the
    /// `Listener-Delay-Secs` header is valid, the body is a valid live value
    /// payload, and the publish rate limit allows the request. The caller
    /// gives `listener_delay_secs` as the result of the header check, so a
    /// bad header never uses a publish slot (ADR 0004 §The Delay).
    ///
    /// SSE and the stored snapshot get the update at once, on the instant
    /// timeline. The listener timeline, which Socket.IO and
    /// `GET /remoteValue` read, gets the update at once only when the delay
    /// is 0 and no earlier update still waits. Else the update joins the
    /// pending list, and a later call to `release_listener_updates` moves
    /// it onto the listener timeline (ADR 0004 §Two Timelines, §The Order On
    /// The Listener Timeline, §The Pending Updates).
    ///
    /// # Errors
    ///
    /// Returns `404 event_not_found`, `403 invalid_token`,
    /// `400 invalid_listener_delay`, the error of `body`, or
    /// `429 publish_rate_limited`.
    pub async fn publish_metadata(
        &self,
        event_id: &str,
        token: &str,
        listener_delay_secs: Result<u64, ApiError>,
        body: PublishMetadataRequest,
    ) -> Result<PublishMetadataResponse, ApiError> {
        let (stored, event) = self.get_entry(event_id).await?;
        if !stored.token_hash_matches(&hash_token(token)) {
            return Err(ApiError::new(StatusCode::FORBIDDEN, "invalid_token"));
        }
        let listener_delay_secs = listener_delay_secs?;
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
        event.set_listener_delay_secs(listener_delay_secs);

        // Lock order (ADR 0004, payment safety): `listener_order`, then
        // `inner`. Taking `listener_order` before the snapshot write keeps
        // the snapshot order and the listener order the same. Holding it
        // through the emit below keeps the listener value and the
        // Socket.IO emit in step. This holds even with a concurrent lease
        // expiry, release sweep, or delete of this event.
        let listener_order = event.listener_order.lock().await;
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
        let listener_emit =
            inner.apply_listener_update(&self.inner.config, now, listener_delay_secs, metadata);
        drop(inner);
        let _ = event.sender.send(snapshot);
        if let Some(value) = listener_emit {
            self.emit_socket_remote_value(event_id, &value).await;
        }
        drop(listener_order);

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

    /// Returns the delay of the last accepted publish of an event, in
    /// seconds (ADR 0004 §The Delay). The value is 0 before the first
    /// publish and after a publish with no `Listener-Delay-Secs` header.
    ///
    /// # Errors
    ///
    /// Returns `404 event_not_found`.
    pub async fn listener_delay_secs(&self, event_id: &str) -> Result<u64, ApiError> {
        let event = self.get_event(event_id).await?;
        Ok(event.listener_delay_secs())
    }

    /// Returns the value that Socket.IO and `GET /remoteValue` give now
    /// (ADR 0004 §Two Timelines). It is `{}` before the listener timeline
    /// holds an update. `register_socket_namespaces` reads this same value
    /// on connect, so a client that connects while an update waits gets
    /// this value, not the newest published payload.
    ///
    /// # Errors
    ///
    /// Returns `404 event_not_found`.
    pub async fn listener_value(&self, event_id: &str) -> Result<Value, ApiError> {
        let event = self.get_event(event_id).await?;
        Ok(event.inner.read().await.listener_value.clone())
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
    /// `{"track": null}` when it has none (ADR 0003). The body also has
    /// `seq`, the display sequence number of that state. It is the `id` of
    /// the same state on `/display/events`. The relay reads the state and
    /// `seq` under one lock.
    ///
    /// # Errors
    ///
    /// Returns `404 event_not_found` or `409 event_not_reserved`.
    pub async fn display_state(&self, event_id: &str) -> Result<Value, ApiError> {
        let (_, event) = self.get_reserved_entry(event_id).await?;
        let display = event.display.read().await;
        let mut body = display.state.clone();
        if let Some(object) = body.as_object_mut() {
            object.insert("seq".to_owned(), Value::from(display.seq));
        }
        Ok(body)
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

    /// Returns the open and new connection counter of `client_addr` (ADR
    /// 0007), making one when the address has none.
    ///
    /// Making one for an address the map does not hold is the point at
    /// which the map can grow. When its length has reached
    /// `ClientLimits::sweep_at`, this runs the sweep of
    /// [`sweep_stale_client_limits`] first, so the map is bounded by the
    /// addresses that have an open connection, plus the addresses that
    /// connected in the current second, amortized over insertions
    /// (AGENTS.md §5). [`RelayState::sweep_client_limits`], on the TTL
    /// reaper's schedule, is a second safety net.
    fn client_counter(&self, client_addr: IpAddr) -> Arc<ClientCounter> {
        let mut limits = self
            .inner
            .client_limits
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(counter) = limits.counters.get(&client_addr) {
            return Arc::clone(counter);
        }

        if limits.counters.len() >= limits.sweep_at {
            let now_window = self.now() & 0xFFFF_FFFF;
            sweep_stale_client_limits(&mut limits.counters, now_window);
            limits.sweep_at = std::cmp::max(limits.floor, 2 * limits.counters.len());
        }

        Arc::clone(
            limits
                .counters
                .entry(client_addr)
                .or_insert_with(|| Arc::new(ClientCounter::new())),
        )
    }

    /// Acquires one listener-connection slot of `client_addr` for an SSE
    /// stream (ADR 0007 §Three Limits).
    ///
    /// Checks the open-connections limit
    /// ([`RelayState::try_acquire_client_open_slot`]), then the new-
    /// connections-per-second limit
    /// ([`RelayState::try_acquire_client_connect_rate`]). A refused attempt
    /// takes no slot of either count: the open-connections increment rolls
    /// back when the second check fails. The caller releases the slot with
    /// [`RelayState::release_client_slot`] when the connection ends, also
    /// after an error.
    ///
    /// A Socket.IO connection does not use this combined check (ADR 0007 §A
    /// Socket.IO Session With No Namespace). Its Engine.IO handshake checks
    /// only the connect rate
    /// ([`RelayState::try_acquire_socketio_handshake_rate`]), and its
    /// namespace connect checks only the open count
    /// ([`RelayState::try_acquire_client_open_slot`]), so the handshake and
    /// the namespace connect never count the same connection twice against
    /// the connect rate.
    ///
    /// # Errors
    ///
    /// Returns `429 listener_limit_reached`.
    fn try_acquire_client_slot(&self, client_addr: IpAddr) -> Result<Arc<ClientCounter>, ApiError> {
        let counter = self.try_acquire_client_open_slot(client_addr)?;
        if !self.try_acquire_client_connect_rate(&counter) {
            counter.open.fetch_sub(1, Ordering::AcqRel);
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "listener_limit_reached",
            ));
        }
        Ok(counter)
    }

    /// Checks and takes one open-connection slot of `client_addr` (ADR 0007
    /// §Three Limits), with `max_listener_connections_per_client`.
    ///
    /// A refused attempt takes no slot. This is the half of
    /// [`RelayState::try_acquire_client_slot`] that a Socket.IO namespace
    /// connect also uses on its own, because its Engine.IO handshake already
    /// checked the connect rate
    /// ([`RelayState::try_acquire_socketio_handshake_rate`]).
    ///
    /// # Errors
    ///
    /// Returns `429 listener_limit_reached`.
    fn try_acquire_client_open_slot(
        &self,
        client_addr: IpAddr,
    ) -> Result<Arc<ClientCounter>, ApiError> {
        let counter = self.client_counter(client_addr);
        let limit = self.inner.config.max_listener_connections_per_client;
        let previous = counter.open.fetch_add(1, Ordering::AcqRel);
        if limit != 0 && previous >= limit {
            counter.open.fetch_sub(1, Ordering::AcqRel);
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "listener_limit_reached",
            ));
        }
        Ok(counter)
    }

    /// Checks and takes one slot of `counter`'s new-connections-per-second
    /// window (ADR 0007 §Three Limits), with
    /// `max_listener_connects_per_client_per_sec`.
    ///
    /// This is the other half of [`RelayState::try_acquire_client_slot`].
    /// [`RelayState::try_acquire_socketio_handshake_rate`] also calls it, for
    /// an Engine.IO handshake that has not joined any namespace yet and so
    /// has no [`ClientCounter`] of its own to roll back.
    fn try_acquire_client_connect_rate(&self, counter: &ClientCounter) -> bool {
        let now = self.now();
        try_acquire_window_slot(
            &counter.connect_window,
            now,
            self.inner.config.max_listener_connects_per_client_per_sec,
        )
    }

    /// Checks and takes one connect-rate slot of `client_addr` for an
    /// Engine.IO handshake (ADR 0007 §A Socket.IO Session With No
    /// Namespace).
    ///
    /// A handshake takes no open-connection slot: only a later namespace
    /// connect, through [`RelayState::try_acquire_client_open_slot`], takes
    /// one. This keeps the handshake and the namespace connect from
    /// counting the same connection twice against the connect rate.
    fn try_acquire_socketio_handshake_rate(&self, client_addr: IpAddr) -> bool {
        let counter = self.client_counter(client_addr);
        self.try_acquire_client_connect_rate(&counter)
    }

    /// Checks and takes one of the `max_socketio_connections` slots of the
    /// full relay (ADR 0007 §Three Limits).
    ///
    /// A refused attempt takes no slot. The caller releases an acquired slot
    /// with [`RelayState::release_socketio_slot`] on every kind of
    /// disconnect.
    fn try_acquire_socketio_slot(&self) -> bool {
        let previous = self
            .inner
            .socketio_connections
            .fetch_add(1, Ordering::AcqRel);
        if previous >= self.inner.config.max_socketio_connections {
            self.inner
                .socketio_connections
                .fetch_sub(1, Ordering::AcqRel);
            return false;
        }
        true
    }

    /// Releases one of the `max_socketio_connections` slots of the full
    /// relay (ADR 0007).
    fn release_socketio_slot(&self) {
        self.inner
            .socketio_connections
            .fetch_sub(1, Ordering::AcqRel);
    }

    /// Releases one listener-connection slot of `client_addr` (ADR 0007).
    ///
    /// Removes the counter of `client_addr` when it then has no open
    /// connection and no connect in the current second. This is an
    /// opportunistic prune only: an address whose one connection opens and
    /// closes inside the same second leaves no later release to run this
    /// check. [`RelayState::client_counter`] and
    /// [`RelayState::sweep_client_limits`] are the two sweeps that bound
    /// the map for that case.
    fn release_client_slot(&self, client_addr: IpAddr, counter: &Arc<ClientCounter>) {
        counter.open.fetch_sub(1, Ordering::AcqRel);
        if counter.open.load(Ordering::Acquire) != 0 {
            return;
        }
        // The window of the last connect packs into the high 32 bits of
        // `connect_window`, the same encoding as `try_acquire_window_slot`.
        let now_window = self.now() & 0xFFFF_FFFF;
        let last_window = counter.connect_window.load(Ordering::Acquire) >> 32;
        if last_window == now_window {
            return;
        }
        let mut limits = self
            .inner
            .client_limits
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if limits
            .counters
            .get(&client_addr)
            .is_some_and(|entry| entry.open.load(Ordering::Acquire) == 0)
        {
            limits.counters.remove(&client_addr);
        }
    }

    /// Removes the counter of each client address that has no open
    /// connection and no connect in the current second (ADR 0007 §Three
    /// Limits).
    ///
    /// [`RelayState::release_client_slot`] prunes an address
    /// opportunistically when its last connection closes, and
    /// [`RelayState::client_counter`] sweeps the map, amortized, when a
    /// new address makes it grow. An address whose one connection opens
    /// and closes inside one second, with no later insertion to trigger
    /// the amortized sweep, can still wait for this sweep. The TTL reaper
    /// calls it on an hourly schedule by default, so it is a second
    /// safety net, not the only bound on the map (AGENTS.md §5).
    fn sweep_client_limits(&self) {
        let now_window = self.now() & 0xFFFF_FFFF;
        let mut limits = self
            .inner
            .client_limits
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        sweep_stale_client_limits(&mut limits.counters, now_window);
        // A flood can leave a high threshold. Reset it to the size after
        // the sweep, as the sweep in `client_counter` does.
        limits.sweep_at = std::cmp::max(limits.floor, 2 * limits.counters.len());
    }

    /// Test-only: sets the amortized-sweep floor and threshold of the ADR
    /// 0007 client-limiter map, so a test can reach the amortized sweep of
    /// [`RelayState::client_counter`] without opening
    /// `CLIENT_LIMITS_SWEEP_FLOOR` connections. Production always starts
    /// at `CLIENT_LIMITS_SWEEP_FLOOR`.
    #[cfg(test)]
    fn set_client_limits_sweep_floor(&self, floor: usize) {
        let mut limits = self
            .inner
            .client_limits
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        limits.floor = floor;
        limits.sweep_at = floor;
    }

    /// Test-only: reads the Socket.IO `connect_timeout` that `app(state)`
    /// gave the attached `SocketIo` (ADR 0007 §A Socket.IO Session With No
    /// Namespace), so a test can check the builder value of the production
    /// path without a real 10-second wait.
    ///
    /// # Panics
    ///
    /// Panics when no `SocketIo` is attached yet, that is, before `app` runs.
    #[cfg(test)]
    fn socketio_connect_timeout(&self) -> Duration {
        self.inner
            .socket_io
            .get()
            .expect("socket_io attached by app(state)")
            .config()
            .connect_timeout
    }

    /// Subscribes to the display stream of a reserved event (ADR 0003),
    /// behind the listener-connection limits of `client_addr` (ADR 0007).
    ///
    /// The client-connection slot is acquired before the global SSE slot,
    /// so a refused connection never takes the global slot either (ADR
    /// 0007 §Three Limits).
    ///
    /// # Errors
    ///
    /// Returns `429 listener_limit_reached` above a limit of `client_addr`,
    /// `503 max_sse_connections_reached` above the global limit, or the
    /// error of [`RelayState::get_reserved_entry`].
    async fn subscribe_display(
        &self,
        event_id: &str,
        last_event_id: Option<u64>,
        client_addr: IpAddr,
    ) -> Result<DisplaySubscription, ApiError> {
        let client_counter = self.try_acquire_client_slot(client_addr)?;

        let previous = self.inner.sse_connections.fetch_add(1, Ordering::AcqRel);
        if previous >= self.inner.config.max_sse_connections {
            self.inner.sse_connections.fetch_sub(1, Ordering::AcqRel);
            self.release_client_slot(client_addr, &client_counter);
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "max_sse_connections_reached",
            ));
        }

        let event = match self.get_reserved_entry(event_id).await {
            Ok((_, event)) => event,
            Err(err) => {
                self.inner.sse_connections.fetch_sub(1, Ordering::AcqRel);
                self.release_client_slot(client_addr, &client_counter);
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
            client_addr,
            client_counter,
        })
    }

    /// Subscribes to the live value stream of an event (ADR 0004), behind
    /// the listener-connection limits of `client_addr` (ADR 0007).
    ///
    /// The client-connection slot is acquired before the global SSE slot,
    /// so a refused connection never takes the global slot either (ADR
    /// 0007 §Three Limits).
    ///
    /// # Errors
    ///
    /// Returns `429 listener_limit_reached` above a limit of `client_addr`,
    /// `503 max_sse_connections_reached` above the global limit, or
    /// `404 event_not_found`.
    async fn subscribe(
        &self,
        event_id: &str,
        last_event_id: Option<u64>,
        client_addr: IpAddr,
    ) -> Result<EventSubscription, ApiError> {
        let client_counter = self.try_acquire_client_slot(client_addr)?;

        let previous = self.inner.sse_connections.fetch_add(1, Ordering::AcqRel);
        if previous >= self.inner.config.max_sse_connections {
            self.inner.sse_connections.fetch_sub(1, Ordering::AcqRel);
            self.release_client_slot(client_addr, &client_counter);
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "max_sse_connections_reached",
            ));
        }

        let event = match self.get_event(event_id).await {
            Ok(event) => event,
            Err(err) => {
                self.inner.sse_connections.fetch_sub(1, Ordering::AcqRel);
                self.release_client_slot(client_addr, &client_counter);
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
            client_addr,
            client_counter,
        })
    }

    /// Removes each ephemeral event whose last activity is older than the
    /// idle TTL, and sweeps the client-limiter map of ADR 0007.
    ///
    /// The reaper never removes a reserved item (ADR 0001). It reads the
    /// class from the store under the table lock, and it takes no event lock.
    /// Because it keeps each reserved item, it never deletes a row of the
    /// state file.
    ///
    /// [`RelayState::sweep_client_limits`] runs first, and unconditionally,
    /// so a client-limiter entry still leaves the map on this reaper's
    /// schedule even when the event table holds nothing to expire.
    ///
    /// Returns the count of removed events.
    pub async fn cleanup_expired(&self) -> usize {
        self.sweep_client_limits();
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

            // Lock order (ADR 0004, payment safety): `listener_order`,
            // then `inner`.
            let listener_order = event.listener_order.lock().await;
            let mut inner = event.inner.write().await;
            // Check the lease again under the write lock. A publish renews the
            // lease before it takes this lock, so a publish that arrived after
            // the first check is visible here, and its snapshot stays.
            let renewed_at = event.renewed_at.load(Ordering::Relaxed);
            if now.saturating_sub(renewed_at) < lease_secs {
                drop(inner);
                drop(listener_order);
                continue;
            }
            if inner.latest.is_none() {
                // No snapshot to clear. The lease still ended, so the display
                // state clears (ADR 0003). This covers an event that published
                // a display state and no payload, for example after a restart.
                drop(inner);
                drop(listener_order);
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

            // The `{}` joins the listener timeline with the delay of the
            // last publish, so it never arrives before the last block
            // (ADR 0004 §The Lease And The `404`).
            let delay = event.listener_delay_secs();
            let listener_emit =
                inner.apply_listener_update(&self.inner.config, now, delay, json!({}));
            drop(inner);

            let _ = event.sender.send(snapshot);
            if let Some(value) = listener_emit {
                self.emit_socket_remote_value(&event_id, &value).await;
            }
            drop(listener_order);

            self.clear_display_after_lease(&event, lease_secs, now)
                .await;
            expired_count += 1;
        }

        expired_count
    }

    /// Moves each pending update whose release time is not after now onto
    /// the listener value of its event, in order, and emits each one on
    /// Socket.IO (ADR 0004 §The Order On The Listener Timeline).
    ///
    /// This follows the lock rule of `expire_leases`: it copies the
    /// candidate events, takes one event lock at a time, and releases the
    /// lock before each emit. For each event it also takes that event's
    /// `listener_order` lock before `inner`. It keeps that lock until the
    /// emits of that event are sent. This keeps a concurrent publish,
    /// lease expiry, or delete of the same event from interleaving with
    /// this sweep (ADR 0004, payment safety).
    ///
    /// `spawn_lease_task` calls this each second, after `expire_leases`.
    ///
    /// Returns the count of released updates, over every event.
    pub async fn release_listener_updates(&self) -> usize {
        let now = self.now();

        // Copy the candidates, then release the table lock before the
        // first event lock.
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

        let mut released_count = 0;
        for (event_id, event) in candidates {
            // Lock order (ADR 0004, payment safety): `listener_order`,
            // then `inner`.
            let listener_order = event.listener_order.lock().await;
            let released: Vec<Value> = {
                let mut inner = event.inner.write().await;
                let mut released = Vec::new();
                while inner
                    .pending_listener_updates
                    .front()
                    .is_some_and(|update| update.release_at <= now)
                {
                    let update = inner
                        .pending_listener_updates
                        .pop_front()
                        .expect("the front was checked above");
                    inner.listener_value = update.value.clone();
                    released.push(update.value);
                }
                released
            };
            for value in released {
                self.emit_socket_remote_value(&event_id, &value).await;
                released_count += 1;
            }
            drop(listener_order);
        }

        released_count
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

/// Counts the open and the new listener connections of one client address
/// (ADR 0007). The relay keeps one of these for each client address that
/// has a recent listener connection.
struct ClientCounter {
    open: AtomicUsize,
    /// The window and the count of a `try_acquire_window_slot` limiter, in
    /// the same packed encoding as that function uses.
    connect_window: AtomicU64,
}

impl ClientCounter {
    fn new() -> Self {
        Self {
            open: AtomicUsize::new(0),
            connect_window: AtomicU64::new(0),
        }
    }
}

/// The client counters of ADR 0007, and the amortized-sweep threshold that
/// bounds the map (AGENTS.md §5).
struct ClientLimits {
    counters: HashMap<IpAddr, Arc<ClientCounter>>,
    /// [`RelayState::client_counter`] sweeps the map, with
    /// [`sweep_stale_client_limits`], once its length reaches this value.
    /// Starts at `floor`. After a sweep, this becomes `max(floor, 2 *` the
    /// length right after that sweep`)`, so the next sweep is amortized
    /// over roughly as many insertions as the map held live entries.
    sweep_at: usize,
    /// The lowest value `sweep_at` can take. `CLIENT_LIMITS_SWEEP_FLOOR`
    /// in production. A test lowers it with
    /// [`RelayState::set_client_limits_sweep_floor`] to reach the
    /// amortized sweep without opening thousands of connections.
    floor: usize,
}

impl ClientLimits {
    fn new() -> Self {
        Self {
            counters: HashMap::new(),
            sweep_at: CLIENT_LIMITS_SWEEP_FLOOR,
            floor: CLIENT_LIMITS_SWEEP_FLOOR,
        }
    }
}

/// Removes every counter of `counters` that has no open connection and no
/// connect in `now_window` (ADR 0007 §Three Limits).
///
/// [`RelayState::client_counter`] (amortized, on growth) and
/// [`RelayState::sweep_client_limits`] (the TTL reaper's schedule) share
/// this one rule, so the condition that bounds the client-limiter map
/// exists in one place.
fn sweep_stale_client_limits(counters: &mut HashMap<IpAddr, Arc<ClientCounter>>, now_window: u64) {
    counters.retain(|_, counter| {
        let open = counter.open.load(Ordering::Acquire) != 0;
        let last_window = counter.connect_window.load(Ordering::Acquire) >> 32;
        open || last_window == now_window
    });
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
    /// The delay of the last accepted publish, in whole seconds (ADR 0004
    /// §The Delay). It starts at 0. A keepalive does not change it. It is
    /// in memory only, so a restart clears it, the same as the snapshot.
    listener_delay_secs: AtomicU64,
    inner: RwLock<LiveEventInner>,
    /// Orders every change to the listener timeline of this event
    /// (`inner.listener_value` and `inner.pending_listener_updates`)
    /// together with the Socket.IO emit that reports the change (ADR 0004,
    /// payment safety).
    ///
    /// Lock order: `listener_order`, then `inner`. Never take
    /// `listener_order` while holding `inner` or the table lock
    /// (`RelayStateInner::events`). A writer takes `listener_order` first,
    /// then `inner`. It changes the listener state and drops `inner`. It
    /// emits on Socket.IO, and only then drops `listener_order`.
    ///
    /// Holding `listener_order` across the emit is the purpose of this
    /// lock. It keeps the listener value and the Socket.IO emit in the
    /// same order, for every writer of this event: a publish, a lease
    /// expiry, the release sweep, and a delete. The rule "never hold a
    /// lock across an emit" still applies to the table lock, `inner`, and
    /// `display`.
    listener_order: tokio::sync::Mutex<()>,
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
    /// The value that Socket.IO and `GET /remoteValue` give now (ADR 0004
    /// §Two Timelines). It starts as `{}`. Only a publish with no delay and
    /// no waiting update, or `RelayState::release_listener_updates`, change
    /// it.
    listener_value: Value,
    /// The updates that wait to reach the listener timeline, oldest first
    /// (ADR 0004 §The Pending Updates). Each release time is never earlier
    /// than the release time of the update before it, so the order of the
    /// instant timeline is kept. It holds at most
    /// `max_pending_listener_updates` updates.
    pending_listener_updates: VecDeque<PendingListenerUpdate>,
}

/// One update that waits to reach the listener timeline (ADR 0004 §The
/// Pending Updates). It is in memory only, the same as the pending list
/// that holds it.
struct PendingListenerUpdate {
    /// The time the update reaches the listener timeline.
    release_at: u64,
    /// The live value of the update.
    value: Value,
}

impl LiveEventInner {
    /// Applies one update to the listener timeline (ADR 0004 §Two
    /// Timelines, §The Order On The Listener Timeline, §The Pending
    /// Updates). Only one function implements the publish rule. A publish
    /// and a lease expiry call it, and nothing else does.
    ///
    /// With `delay` 0 and no pending update, this sets the listener value
    /// and returns it, so the caller emits it on Socket.IO. Else it
    /// computes a release time of `now` plus `delay`. That time is never
    /// earlier than the release time of the newest pending update. It
    /// joins the pending list with that time, and returns `None`.
    ///
    /// When the pending list already holds
    /// `config.max_pending_listener_updates` updates, this update
    /// replaces the newest one instead of growing the list. It keeps the
    /// later release time of the two.
    ///
    /// The caller holds this event's `inner` write lock and its
    /// `listener_order` lock. It keeps holding `listener_order` through
    /// the emit of the returned value. This keeps the listener order and
    /// the Socket.IO emit order from separating (ADR 0004, payment
    /// safety).
    fn apply_listener_update(
        &mut self,
        config: &AppConfig,
        now: u64,
        delay: u64,
        value: Value,
    ) -> Option<Value> {
        if delay == 0 && self.pending_listener_updates.is_empty() {
            // Fast path (ADR 0004 §Two Timelines): no delay and nothing
            // waits, so the update reaches the listener timeline at once.
            self.listener_value = value.clone();
            return Some(value);
        }

        // The order rule (ADR 0004 §The Order On The Listener Timeline): a
        // release time is never earlier than the release time of the
        // update before it.
        let release_at = now.saturating_add(delay);
        let release_at = self
            .pending_listener_updates
            .back()
            .map_or(release_at, |newest| release_at.max(newest.release_at));

        if self.pending_listener_updates.len() < config.max_pending_listener_updates {
            self.pending_listener_updates
                .push_back(PendingListenerUpdate { release_at, value });
        } else {
            // The bound (ADR 0004 §The Pending Updates): the new update
            // replaces the newest pending update, and keeps the later
            // release time of the two. `AppConfig::from_env` rejects a
            // bound below 1, so the list is not empty here.
            let newest = self
                .pending_listener_updates
                .back_mut()
                .expect("max_pending_listener_updates is at least 1");
            newest.release_at = release_at;
            newest.value = value;
        }
        None
    }
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
    /// the present state and the image of the most recent earlier state that
    /// had a relay image.
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
        // "Previous" is the most recent earlier state with a relay image. A
        // state with no relay image, such as `null` or a URL, does not reset
        // it. Else a second such state removes the image of the track that a
        // client behind the stream still shows.
        if let Some(sha256) = artwork_sha256(&self.state) {
            self.previous_sha256 = Some(sha256.to_string());
        }
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
            listener_delay_secs: AtomicU64::new(0),
            inner: RwLock::new(LiveEventInner {
                seq: 0,
                latest: None,
                replay: VecDeque::with_capacity(REPLAY_CAPACITY),
                listener_value: json!({}),
                pending_listener_updates: VecDeque::new(),
            }),
            listener_order: tokio::sync::Mutex::new(()),
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

    /// Returns the delay of the last accepted publish, in seconds (ADR 0004
    /// §The Delay).
    fn listener_delay_secs(&self) -> u64 {
        self.listener_delay_secs.load(Ordering::Relaxed)
    }

    /// Sets the delay of the last accepted publish (ADR 0004 §The Delay).
    fn set_listener_delay_secs(&self, delay: u64) {
        self.listener_delay_secs.store(delay, Ordering::Relaxed);
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
    /// The client address this subscription counts against (ADR 0007).
    client_addr: IpAddr,
    client_counter: Arc<ClientCounter>,
}

impl Drop for EventSubscription {
    fn drop(&mut self) {
        self.state
            .inner
            .sse_connections
            .fetch_sub(1, Ordering::AcqRel);
        self.state
            .release_client_slot(self.client_addr, &self.client_counter);
    }
}

/// A subscriber of `/display/events`. It counts against the same SSE
/// connection limit as `/events`, and the same listener-connection limits
/// of its client address (ADR 0007).
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
    /// The client address this subscription counts against (ADR 0007).
    client_addr: IpAddr,
    client_counter: Arc<ClientCounter>,
}

impl Drop for DisplaySubscription {
    fn drop(&mut self) {
        self.state
            .inner
            .sse_connections
            .fetch_sub(1, Ordering::AcqRel);
        self.state
            .release_client_slot(self.client_addr, &self.client_counter);
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
    // `artist`, `title` and `artwork` are required. `songLine`, `value` and
    // `album` are optional (ADR 0005, ADR 0006). Any other key makes the state invalid.
    let known_keys = ["artist", "title", "artwork", "songLine", "value", "album"];
    if !track.keys().all(|key| known_keys.contains(&key.as_str())) {
        return Err(invalid());
    }
    if !track.get("artist").is_some_and(Value::is_string) {
        return Err(invalid());
    }
    if !track.get("title").is_some_and(Value::is_string) {
        return Err(invalid());
    }
    if let Some(song_line) = track.get("songLine")
        && !is_text_of_length(song_line, MAX_SONG_LINE_CHARS)
    {
        return Err(invalid());
    }
    if let Some(value) = track.get("value") {
        let value = value.as_object().ok_or_else(invalid)?;
        let valid = value.len() == 2
            && ["eventGuid", "blockGuid"].iter().all(|key| {
                value
                    .get(*key)
                    .is_some_and(|guid| is_text_of_length(guid, MAX_VALUE_GUID_CHARS))
            });
        if !valid {
            return Err(invalid());
        }
    }
    if let Some(album) = track.get("album")
        && !is_text_of_length(album, MAX_ALBUM_CHARS)
    {
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

/// Returns `true` for a string of 1 to `max_chars` characters.
fn is_text_of_length(value: &Value, max_chars: usize) -> bool {
    value
        .as_str()
        .is_some_and(|text| (1..=max_chars).contains(&text.chars().count()))
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
    let (socket_layer, io) = SocketIo::builder()
        .connect_timeout(SOCKETIO_CONNECT_TIMEOUT)
        .with_state(state.clone())
        .build_layer();

    register_socket_namespaces(io.clone());
    state.attach_socket_io(io);

    // The handshake-rate middleware wraps the whole router, including the
    // Socket.IO layer below: an Engine.IO request is dispatched to
    // socketioxide by path, before it would ever reach a route (ADR 0007 §A
    // Socket.IO Session With No Namespace).
    let handshake_rate_limit_state = state.clone();

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
        .layer(middleware::from_fn_with_state(
            handshake_rate_limit_state,
            limit_socketio_handshake_rate,
        ))
}

/// Checks the connect-rate limit of ADR 0007 on an Engine.IO handshake,
/// before the request reaches socketioxide.
///
/// This middleware wraps every request to `app(state)`, because a Socket.IO
/// request never reaches an axum route: the Socket.IO layer dispatches it
/// by path first. A request that is not a handshake
/// ([`is_socketio_handshake`]) passes through unchanged.
///
/// A handshake above the limit gets `429 listener_limit_reached` here and
/// never reaches socketioxide, so no Engine.IO session is created for it.
/// This handshake-level check takes no open-connection slot: only a later
/// namespace connect, in [`register_socket_namespaces`], takes one. That
/// keeps the handshake and the namespace connect from counting the same
/// connection twice against the connect rate (ADR 0007 §A Socket.IO
/// Session With No Namespace).
async fn limit_socketio_handshake_rate(
    AxumState(state): AxumState<RelayState>,
    ClientPeerAddr(peer): ClientPeerAddr,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    if is_socketio_handshake(&request) {
        let client_addr =
            client_address(request.headers(), peer, &state.inner.config.trusted_proxies);
        if !state.try_acquire_socketio_handshake_rate(client_addr) {
            return ApiError::new(StatusCode::TOO_MANY_REQUESTS, "listener_limit_reached")
                .into_response();
        }
    }
    next.run(request).await
}

/// True for a request that engineioxide treats as a new Engine.IO session: a
/// request to `SOCKET_IO_REQ_PATH` with no `sid` query parameter (ADR 0007
/// §A Socket.IO Session With No Namespace). A request with a `sid`
/// continues an existing session (a poll or a post), and does not open a
/// new one.
fn is_socketio_handshake(request: &axum::extract::Request) -> bool {
    if !request.uri().path().starts_with(SOCKET_IO_REQ_PATH) {
        return false;
    }
    let query = request.uri().query().unwrap_or("");
    !query.split('&').any(|part| part.starts_with("sid="))
}

/// Spawns the background task that expires inactive ephemeral events and
/// sweeps the client-limiter map of ADR 0007, on a tick between one minute
/// and one hour.
///
/// The tick is `EVENT_TTL_SECS / 4`, clamped to that range. With the
/// default TTL of 24 hours, the tick is one hour. [`RelayState::client_counter`]
/// also sweeps the map, amortized over insertions, each time it grows
/// enough to matter; see [`RelayState::sweep_client_limits`]. This task's
/// tick is the backstop: the longest a stale address can stay in the map
/// when the map never grows enough to trigger that amortized sweep.
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

/// Spawns the background task that expires leases and releases pending
/// listener updates each second.
///
/// The task calls [`RelayState::expire_leases`], then
/// [`RelayState::release_listener_updates`] (ADR 0004), on a one-second tick
/// for the life of the process.
pub fn spawn_lease_task(state: RelayState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));

        loop {
            ticker.tick().await;
            let expired = state.expire_leases().await;
            if expired > 0 {
                tracing::info!(expired, "expired live leases");
            }
            let released = state.release_listener_updates().await;
            if released > 0 {
                tracing::info!(released, "released pending listener updates");
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

/// Handles `POST /v1/liveitems/{event_id}/metadata`.
///
/// The `Listener-Delay-Secs` header carries the broadcaster's listener
/// delay (ADR 0004). This handler reads it and gives the result to
/// `RelayState::publish_metadata`, which checks it after the token check.
async fn publish_metadata(
    AxumState(state): AxumState<RelayState>,
    Path(event_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<PublishMetadataRequest>,
) -> Result<Json<PublishMetadataResponse>, ApiError> {
    let token = bearer_token(&headers)?;
    let listener_delay_secs =
        parse_listener_delay_header(&headers, state.inner.config.max_listener_delay_secs);
    Ok(Json(
        state
            .publish_metadata(&event_id, token, listener_delay_secs, body)
            .await?,
    ))
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
    ClientPeerAddr(peer): ClientPeerAddr,
) -> Result<Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let last_event_id = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let client_addr = client_address(&headers, peer, &state.inner.config.trusted_proxies);

    let subscription = state
        .subscribe_display(&event_id, last_event_id, client_addr)
        .await?;

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

/// Handles `GET /v1/liveitems/{event_id}/remoteValue`.
///
/// This route follows the listener timeline, not the instant timeline
/// (ADR 0004 §Two Timelines). It gives the same value as the last
/// Socket.IO `remoteValue` emit of this event.
async fn remote_value(
    AxumState(state): AxumState<RelayState>,
    Path(event_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(state.listener_value(&event_id).await?))
}

/// Handles `GET /v1/liveitems/{event_id}/events`.
///
/// It is an SSE stream of `remoteValue` events on the instant timeline
/// (ADR 0004). The client-connection limits of ADR 0007 apply to it, the
/// same as to `/display/events`.
async fn events(
    AxumState(state): AxumState<RelayState>,
    Path(event_id): Path<String>,
    headers: HeaderMap,
    ClientPeerAddr(peer): ClientPeerAddr,
) -> Result<Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let last_event_id = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let client_addr = client_address(&headers, peer, &state.inner.config.trusted_proxies);

    let subscription = state
        .subscribe(&event_id, last_event_id, client_addr)
        .await?;

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

/// Releases the two Socket.IO slots of ADR 0007 (the global slot and the
/// client-address open-connection slot) exactly once, however many times
/// [`SocketioSlotRelease::release`] runs on this guard or on a clone of it.
///
/// `register_socket_namespaces` gives a clone of the same guard to the
/// registered `on_disconnect` handler and to its own check of
/// `SocketRef::connected()` right after registering it. Both can try the
/// release for the same connection, because socketioxide spawns the
/// connect handler as its own task
/// (`socketioxide::handler::connect::ConnectHandler::call`,
/// `tokio::spawn(fut)`) after it has already added the socket to its
/// namespace (`socketioxide::ns::Namespace::connect`): a disconnect can
/// close that socket before the spawned task ever runs, so the
/// `connected()` check is what catches a release the handler missed. The
/// `AtomicBool` makes the first caller of either path the only one that
/// runs the release.
#[derive(Clone)]
struct SocketioSlotRelease {
    state: RelayState,
    client_addr: IpAddr,
    client_counter: Arc<ClientCounter>,
    released: Arc<AtomicBool>,
}

impl SocketioSlotRelease {
    fn new(state: RelayState, client_addr: IpAddr, client_counter: Arc<ClientCounter>) -> Self {
        Self {
            state,
            client_addr,
            client_counter,
            released: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Releases the slots, unless an earlier call on this guard or a clone
    /// of it already did.
    fn release(&self) {
        if self.released.swap(true, Ordering::AcqRel) {
            return;
        }
        self.state.release_socketio_slot();
        self.state
            .release_client_slot(self.client_addr, &self.client_counter);
    }
}

/// Registers the `/event` namespace, behind the Socket.IO limits of ADR
/// 0007.
///
/// The handler checks the limits in this order (ADR 0007 §Three Limits):
/// the global Socket.IO limit
/// ([`RelayState::try_acquire_socketio_slot`]), then the open count of the
/// client address ([`RelayState::try_acquire_client_open_slot`]), then the
/// event. A refused connection gets `{}` on `remoteValue` and a disconnect,
/// as for an unknown event, and takes no slot: each check rolls back the
/// slots an earlier check took before it refuses.
///
/// [`SocketRef::on_disconnect`] releases both slots through a
/// [`SocketioSlotRelease`], and it is registered right after both are
/// acquired, before the event check and before any `.await`.
/// `socketioxide::ns::Namespace::connect` already added this socket to its
/// namespace before calling this handler, so a disconnect from outside
/// this handler (ADR 0007's own `disconnect_socket_clients`, a lost
/// transport, and so on) can reach and close this socket while this
/// handler still runs, or even before this handler's task is scheduled to
/// run at all. Registering the release before any later check or `.await`
/// closes the first of those gaps: with no handler registered yet, such a
/// close would otherwise remove the socket with no release, leaking both
/// slots forever (`socketioxide::socket::Socket::close`).
///
/// The second gap, a disconnect that beats this handler's own task to the
/// scheduler, needs the `connected()` check right after registering the
/// handler: see [`SocketioSlotRelease`]. Once past that check, every later
/// refusal only calls `refuse`, which emits `{}` and disconnects; the
/// registered handler is what then releases the slots.
///
/// This handler takes no part in the connect-rate limit
/// (`MAX_LISTENER_CONNECTS_PER_CLIENT_PER_SEC`): the Engine.IO handshake
/// middleware ([`limit_socketio_handshake_rate`]) already checked it, so
/// counting it again here would count one connection twice.
fn register_socket_namespaces(io: SocketIo) {
    io.ns(
        "/event",
        async |socket: SocketRef, SocketState(state): SocketState<RelayState>| {
            let refuse = |socket: &SocketRef| {
                let _ = socket.emit("remoteValue", &json!({}));
                socket.clone().disconnect().ok();
            };

            if !state.try_acquire_socketio_slot() {
                refuse(&socket);
                return;
            }

            let peer = socket_peer_addr(&socket);
            let client_addr = client_address(
                &socket.req_parts().headers,
                peer,
                &state.inner.config.trusted_proxies,
            );
            // Only the global slot is held so far: a refusal here rolls it
            // back directly, because no disconnect handler exists yet to
            // roll it back for this call.
            let client_counter = match state.try_acquire_client_open_slot(client_addr) {
                Ok(counter) => counter,
                Err(_) => {
                    state.release_socketio_slot();
                    refuse(&socket);
                    return;
                }
            };

            // Both slots are held now. Register their release before the
            // event check and before the `.await` below, so a disconnect
            // concurrent with either one still releases them (see the
            // function doc comment).
            let release = SocketioSlotRelease::new(state.clone(), client_addr, client_counter);

            let on_disconnect_release = release.clone();
            socket.on_disconnect(move || async move {
                on_disconnect_release.release();
            });

            // `Socket::close` sets `connected` to `false` before it takes
            // the registered handler (see `SocketioSlotRelease`'s doc
            // comment). A disconnect that beat the registration above, and
            // so found no handler to call, has already made this `false`
            // by the time this reads it, and this call is then the only
            // release that ever runs for this connection. A disconnect
            // that instead found the handler calls it, which releases the
            // slots on its own; the guard's `AtomicBool` keeps the two
            // paths from both releasing.
            if !socket.connected() {
                release.release();
                return;
            }

            let Some(event_id) = socket_event_id(&socket) else {
                refuse(&socket);
                return;
            };

            // The initial emit reads the listener value through the same
            // function as `GET /remoteValue` (ADR 0004 §Two Timelines), so
            // a client that connects while an update waits gets the
            // listener value, not the newest published payload.
            let current = match state.listener_value(&event_id).await {
                Ok(current) => current,
                Err(_) => {
                    refuse(&socket);
                    return;
                }
            };

            socket.join(event_id.clone());

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

/// Reads and checks the `Listener-Delay-Secs` header of a publish (ADR 0004
/// §The Delay).
///
/// A request with no such header has the delay 0. The handler calls this
/// before it calls `RelayState::publish_metadata`, which applies the
/// result after its token check, so a bad header never uses a publish slot.
///
/// # Errors
///
/// Returns `400 invalid_listener_delay` for more than one header, a header
/// value that is not ASCII text, an empty value, a value that is not a
/// whole number, and a value over `max_listener_delay_secs`.
fn parse_listener_delay_header(
    headers: &HeaderMap,
    max_listener_delay_secs: u64,
) -> Result<u64, ApiError> {
    let invalid = || ApiError::new(StatusCode::BAD_REQUEST, "invalid_listener_delay");

    let mut values = headers.get_all(LISTENER_DELAY_HEADER).iter();
    let Some(value) = values.next() else {
        return Ok(0);
    };
    if values.next().is_some() {
        return Err(invalid());
    }

    let text = value.to_str().map_err(|_| invalid())?;
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    let delay: u64 = text.parse().map_err(|_| invalid())?;
    if delay > max_listener_delay_secs {
        return Err(invalid());
    }
    Ok(delay)
}

/// Logged once, the first time a request carries no `ConnectInfo` (ADR
/// 0007 §The Client Address).
static MISSING_CONNECT_INFO_LOGGED: Once = Once::new();

/// Reads the TCP peer address from the extensions of a request: the real
/// `ConnectInfo<SocketAddr>` that `axum::serve`'s
/// `into_make_service_with_connect_info` inserts, or the
/// `MockConnectInfo<SocketAddr>` a test layers in its place (ADR 0007 §The
/// Client Address). `None` with neither extension present.
///
/// [`ClientPeerAddr`] (an HTTP route) and [`socket_peer_addr`] (the
/// Socket.IO `/event` handler) both call this, so the one rule that picks
/// the peer address out of a request's extensions lives here. A
/// `socketioxide::SocketRef::req_parts()` gives only `&http::request::Parts`,
/// not `&mut`, so neither caller can run the `ConnectInfo` extractor's own
/// async `FromRequestParts` implementation; this function reads the same
/// two extension types it does, synchronously.
fn connect_info_peer_addr(extensions: &http::Extensions) -> Option<IpAddr> {
    extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip())
        .or_else(|| {
            extensions
                .get::<axum::extract::connect_info::MockConnectInfo<SocketAddr>>()
                .map(|axum::extract::connect_info::MockConnectInfo(addr)| addr.ip())
        })
}

/// The TCP peer address of an HTTP request, from
/// [`connect_info_peer_addr`].
///
/// With no `ConnectInfo` in the request extensions, the address is
/// `0.0.0.0`, and the relay logs this once. This keeps a test that gives
/// no peer address working, and keeps `axum::serve` the only production
/// path that can give a real peer address.
struct ClientPeerAddr(IpAddr);

impl<S> FromRequestParts<S> for ClientPeerAddr
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        match connect_info_peer_addr(&parts.extensions) {
            Some(addr) => Ok(Self(addr)),
            None => {
                MISSING_CONNECT_INFO_LOGGED.call_once(|| {
                    tracing::warn!(
                        client_addr = %NO_CONNECT_INFO_ADDR,
                        "no ConnectInfo in the request; using the default client address"
                    );
                });
                Ok(Self(NO_CONNECT_INFO_ADDR))
            }
        }
    }
}

/// The TCP peer address of a Socket.IO handshake, from
/// [`connect_info_peer_addr`] applied to `socket.req_parts()` (ADR 0007 §The
/// Client Address), with the same default as [`ClientPeerAddr`] when the
/// handshake request carried no `ConnectInfo`.
fn socket_peer_addr(socket: &SocketRef) -> IpAddr {
    connect_info_peer_addr(&socket.req_parts().extensions).unwrap_or_else(|| {
        MISSING_CONNECT_INFO_LOGGED.call_once(|| {
            tracing::warn!(
                client_addr = %NO_CONNECT_INFO_ADDR,
                "no ConnectInfo in the Socket.IO handshake; using the default client address"
            );
        });
        NO_CONNECT_INFO_ADDR
    })
}

/// Set after the relay logs the warning of ADR 0007 §A Warning For An
/// Ignored Header, amended 2026-10-18. [`should_warn_about_ignored_x_real_ip`]
/// reads and sets it.
static WARNED_ABOUT_IGNORED_X_REAL_IP: AtomicBool = AtomicBool::new(false);

/// True the first time this runs with `flag`, and false every later time.
///
/// [`client_address`] calls this with [`WARNED_ABOUT_IGNORED_X_REAL_IP`]
/// before it logs the warning of ADR 0007 §A Warning For An Ignored Header,
/// so the warning goes out once for each process. A unit test calls this
/// function with its own flag, so another test that logs the warning cannot
/// change the result.
fn should_warn_about_ignored_x_real_ip(flag: &AtomicBool) -> bool {
    !flag.swap(true, Ordering::SeqCst)
}

/// Finds the client address of a request (ADR 0007 §The Client Address,
/// amended 2026-10-18).
///
/// `peer` is the TCP peer address, in its canonical form
/// ([`IpAddr::to_canonical`]): an IPv4-mapped IPv6 peer, for example
/// `::ffff:127.0.0.1`, becomes the IPv4 address `127.0.0.1`. The client
/// address is the canonical peer, unless the canonical peer is in
/// `trusted_proxies` and the request has exactly one `X-Real-IP` header
/// that holds exactly one correct IP address. In that case the client
/// address is the `X-Real-IP` address. The relay never reads
/// `X-Forwarded-For`.
///
/// When the canonical peer is not in `trusted_proxies` and the request
/// carries an `X-Real-IP` header, this logs one warning for the whole
/// process, through [`should_warn_about_ignored_x_real_ip`]. The warning
/// never gives the value of `X-Real-IP`.
fn client_address(headers: &HeaderMap, peer: IpAddr, trusted_proxies: &[TrustedProxy]) -> IpAddr {
    let peer = peer.to_canonical();
    if !trusted_proxies.iter().any(|proxy| proxy.contains(peer)) {
        if headers.contains_key(X_REAL_IP_HEADER)
            && should_warn_about_ignored_x_real_ip(&WARNED_ABOUT_IGNORED_X_REAL_IP)
        {
            let proxies_text = trusted_proxies
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            tracing::warn!(
                peer = %peer,
                trusted_proxies = %proxies_text,
                "ignoring X-Real-IP from a peer that TRUSTED_PROXIES does not trust"
            );
        }
        return peer;
    }
    let mut values = headers.get_all(X_REAL_IP_HEADER).iter();
    let Some(value) = values.next() else {
        return peer;
    };
    if values.next().is_some() {
        return peer;
    }
    value
        .to_str()
        .ok()
        .and_then(|text| text.parse::<IpAddr>().ok())
        .unwrap_or(peer)
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

    /// Serializes the configuration tests that mutate the process
    /// environment. `cargo test` runs tests of one process on parallel
    /// threads, and `AppConfig::from_env` reads every variable, so one
    /// test's value can leak into another test's read. Each such test
    /// holds this lock for its whole body: the set, the read, and the
    /// remove.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

    /// The count of client addresses the limiter of ADR 0007 is tracking.
    fn client_limit_entries(state: &RelayState) -> usize {
        state
            .inner
            .client_limits
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .counters
            .len()
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
                Ok(0),
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
                Ok(0),
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
                Ok(0),
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
                    Ok(0),
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
                    Ok(0),
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
            .subscribe(&created.event_id, None, NO_CONNECT_INFO_ADDR)
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
                Ok(0),
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
                    Ok(0),
                    PublishMetadataRequest(json!({"x": 1})),
                )
                .await
                .expect("publish under limit");
        }

        let err = state
            .publish_metadata(
                &created.event_id,
                &created.broadcaster_token,
                Ok(0),
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
                Ok(0),
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
            .subscribe(&created.event_id, None, NO_CONNECT_INFO_ADDR)
            .await
            .expect("first subscribe");

        let err = match state
            .subscribe(&created.event_id, None, NO_CONNECT_INFO_ADDR)
            .await
        {
            Ok(_) => panic!("second subscribe should be rejected"),
            Err(err) => err,
        };
        assert_eq!(err.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(err.code, "max_sse_connections_reached");

        drop(sub);
        state
            .subscribe(&created.event_id, None, NO_CONNECT_INFO_ADDR)
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
    async fn cleanup_expired_sweeps_client_limiter_entries_with_no_open_connection() {
        let (state, clock) = test_state(1);
        let created = state.create_event().await.expect("create event");

        // Five client addresses each open and close one stream inside the
        // same second (ADR 0007). The opportunistic prune in
        // `release_client_slot` cannot remove any of them: a connect this
        // same second is still a recent connect.
        for low_byte in 0..5u8 {
            let addr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, low_byte));
            let sub = state
                .subscribe(&created.event_id, None, addr)
                .await
                .expect("subscribe");
            drop(sub);
        }
        assert_eq!(client_limit_entries(&state), 5);

        // A later second: the sweep of `cleanup_expired` removes every
        // entry that has no open connection and no connect in the current
        // second.
        clock.store(2, Ordering::SeqCst);
        state.cleanup_expired().await;
        assert_eq!(client_limit_entries(&state), 0);
    }

    #[tokio::test]
    async fn cleanup_expired_keeps_an_address_with_an_open_connection_and_still_releases_it() {
        let (state, clock) = test_state(1);
        let created = state.create_event().await.expect("create event");
        let addr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 50));

        let sub = state
            .subscribe(&created.event_id, None, addr)
            .await
            .expect("subscribe");

        // The sweep never removes an address with an open connection, even
        // on a later second.
        clock.store(2, Ordering::SeqCst);
        state.cleanup_expired().await;
        assert_eq!(client_limit_entries(&state), 1);

        // The slot still releases correctly once the stream closes.
        drop(sub);
        assert_eq!(client_limit_entries(&state), 0);
    }

    #[tokio::test]
    async fn the_amortized_sweep_bounds_the_client_limiter_map_with_no_ttl_reaper_call() {
        let (state, clock) = test_state(1);
        let created = state.create_event().await.expect("create event");
        // Lower the amortized-sweep floor so the test does not need
        // `CLIENT_LIMITS_SWEEP_FLOOR` (1024) connections to reach it.
        // Production always starts at that constant.
        let floor = 5;
        state.set_client_limits_sweep_floor(floor);

        // Two addresses keep one open connection through the whole test.
        // The sweep, opportunistic or amortized, must never remove an
        // address with an open connection.
        let kept_a = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 200));
        let kept_b = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 201));
        let sub_a = state
            .subscribe(&created.event_id, None, kept_a)
            .await
            .expect("subscribe kept_a");
        let sub_b = state
            .subscribe(&created.event_id, None, kept_b)
            .await
            .expect("subscribe kept_b");

        // Each second, one new, distinct address opens and closes one
        // stream. No call to `cleanup_expired` runs in this test: the
        // amortized sweep inside `RelayState::client_counter` is the only
        // thing that can bound the map here.
        for low_byte in 0..40u8 {
            clock.store(u64::from(low_byte) + 2, Ordering::SeqCst);
            let addr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, low_byte));
            let sub = state
                .subscribe(&created.event_id, None, addr)
                .await
                .expect("subscribe");
            drop(sub);

            // At most one address connected in the current second, and
            // the two kept addresses are always open. The amortized sweep
            // keeps the map from ever growing close to 40 entries.
            let entries = client_limit_entries(&state);
            assert!(
                entries <= 2 * floor + 2,
                "iteration {low_byte}: {entries} entries in the map"
            );
        }

        drop(sub_a);
        drop(sub_b);
    }

    /// A deterministic test of the race itself (a disconnect that beats
    /// the connect handler's spawned task to the scheduler, before
    /// `on_disconnect` is registered) is not practical through `oneshot`:
    /// nothing in the public API of `tower`, `axum`, or `socketioxide`
    /// lets a test control the relative scheduling of two independently
    /// spawned tokio tasks. This test instead proves the guard itself:
    /// [`SocketioSlotRelease::release`] runs the release once, however
    /// many times it is called, on the guard or on a clone of it (ADR
    /// 0007 §A Socket.IO Session With No Namespace).
    #[tokio::test]
    async fn socketio_slot_release_runs_the_release_at_most_once() {
        let (state, _) = test_state(1);
        let client_addr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 210));

        assert!(state.try_acquire_socketio_slot());
        let client_counter = state
            .try_acquire_client_open_slot(client_addr)
            .expect("acquire client open slot");
        assert_eq!(state.inner.socketio_connections.load(Ordering::Acquire), 1);
        assert_eq!(client_counter.open.load(Ordering::Acquire), 1);

        let release = SocketioSlotRelease::new(state.clone(), client_addr, client_counter.clone());

        // Two release calls, through two clones of the guard, the same as
        // the registered `on_disconnect` handler and the post-
        // registration `connected()` check in `register_socket_namespaces`
        // can each attempt for the same connection.
        release.clone().release();
        release.release();

        assert_eq!(state.inner.socketio_connections.load(Ordering::Acquire), 0);
        assert_eq!(client_counter.open.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn publish_response_reports_default_lease_and_keepalive_interval() {
        let (state, _) = test_state(1);
        let created = state.create_event().await.expect("create event");

        let response = state
            .publish_metadata(
                &created.event_id,
                &created.broadcaster_token,
                Ok(0),
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
                Ok(0),
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
                Ok(0),
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
                Ok(0),
                PublishMetadataRequest(json!({"title": "Live"})),
            )
            .await
            .expect("publish");

        clock.store(11, Ordering::SeqCst);
        assert_eq!(state.expire_leases().await, 1);

        let subscription = state
            .subscribe(&created.event_id, Some(published.seq), NO_CONNECT_INFO_ADDR)
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
                Ok(0),
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
                Ok(0),
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
    fn max_pending_listener_updates_that_is_not_a_number_is_a_configuration_error() {
        // SAFETY: `ENV_LOCK` is held for the whole body, so no other test
        // races this mutation of the process environment.
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe {
            std::env::set_var("MAX_PENDING_LISTENER_UPDATES", "not-a-number");
        }
        let result = AppConfig::from_env();
        // SAFETY: see above.
        unsafe {
            std::env::remove_var("MAX_PENDING_LISTENER_UPDATES");
        }

        let err = result.expect_err("a non-numeric value is rejected");
        assert!(err.to_string().contains("MAX_PENDING_LISTENER_UPDATES"));
    }

    #[test]
    fn max_pending_listener_updates_of_zero_is_a_configuration_error() {
        // SAFETY: `ENV_LOCK` is held for the whole body, so no other test
        // races this mutation of the process environment.
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe {
            std::env::set_var("MAX_PENDING_LISTENER_UPDATES", "0");
        }
        let result = AppConfig::from_env();
        // SAFETY: see above.
        unsafe {
            std::env::remove_var("MAX_PENDING_LISTENER_UPDATES");
        }

        let err = result.expect_err("a bound of 0 is rejected");
        assert!(err.to_string().contains("MAX_PENDING_LISTENER_UPDATES"));
    }

    #[test]
    fn lease_secs_below_minimum_is_a_configuration_error() {
        // SAFETY: `ENV_LOCK` is held for the whole body, so no other test
        // races this mutation of the process environment.
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    fn trusted_proxies_that_does_not_parse_is_a_configuration_error() {
        // SAFETY: `ENV_LOCK` is held for the whole body, so no other test
        // races this mutation of the process environment.
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe {
            std::env::set_var("TRUSTED_PROXIES", "127.0.0.1,not-an-ip-address");
        }
        let result = AppConfig::from_env();
        // SAFETY: see above.
        unsafe {
            std::env::remove_var("TRUSTED_PROXIES");
        }

        let err = result.expect_err("an entry that is not an IP address is rejected");
        assert!(err.to_string().contains("TRUSTED_PROXIES"));
    }

    #[test]
    fn trusted_proxies_defaults_to_loopback() {
        assert_eq!(
            AppConfig::for_tests().trusted_proxies,
            vec![
                TrustedProxy {
                    network: IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                    prefix_len: 32,
                },
                TrustedProxy {
                    network: IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
                    prefix_len: 128,
                },
            ]
        );
    }

    /// Amendment 2026-10-18: each of these `TRUSTED_PROXIES` entries is a
    /// configuration error that names `TRUSTED_PROXIES`.
    #[test]
    fn trusted_proxies_rejects_each_incorrect_cidr_entry() {
        for entry in [
            "10.0.0.0/33",
            "::/129",
            "172.18.0.1/16",
            "10.0.0.0/x",
            "10.0.0.0/",
        ] {
            // SAFETY: `ENV_LOCK` is held for the whole body, so no other
            // test races this mutation of the process environment.
            let _guard = ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            unsafe {
                std::env::set_var("TRUSTED_PROXIES", entry);
            }
            let result = AppConfig::from_env();
            // SAFETY: see above.
            unsafe {
                std::env::remove_var("TRUSTED_PROXIES");
            }

            let err = result.expect_err(&format!("{entry:?} is rejected"));
            assert!(err.to_string().contains("TRUSTED_PROXIES"));
        }
    }

    /// Amendment 2026-10-18: a CIDR range holds the addresses of its
    /// prefix, and no other address of its family.
    #[test]
    fn a_cidr_range_holds_only_the_addresses_of_its_prefix() {
        let ipv6_range = parse_trusted_proxy("fd00::/8").expect("valid range");
        assert!(ipv6_range.contains("fd00::1".parse().expect("valid address")));
        assert!(!ipv6_range.contains("fe80::1".parse().expect("valid address")));

        let every_ipv4_address = parse_trusted_proxy("0.0.0.0/0").expect("valid range");
        assert!(every_ipv4_address.contains(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1))));
        assert!(!every_ipv4_address.contains("::1".parse().expect("valid address")));
    }

    /// Amendment 2026-10-18: the relay compares the canonical form of the
    /// peer address, so an IPv4-mapped IPv6 peer matches an IPv4 entry.
    #[test]
    fn an_ipv4_mapped_peer_matches_the_default_trusted_proxies() {
        let peer: IpAddr = "::ffff:127.0.0.1".parse().expect("valid address");
        let mut headers = HeaderMap::new();
        headers.insert(
            X_REAL_IP_HEADER,
            axum::http::HeaderValue::from_static("203.0.113.7"),
        );
        assert_eq!(
            client_address(&headers, peer, &default_trusted_proxies()),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7))
        );
    }

    /// Amendment 2026-10-18: the warning for an ignored `X-Real-IP` goes
    /// out once for each process.
    #[test]
    fn should_warn_about_ignored_x_real_ip_is_true_only_on_the_first_call() {
        let flag = AtomicBool::new(false);
        assert!(should_warn_about_ignored_x_real_ip(&flag));
        for _ in 0..3 {
            assert!(!should_warn_about_ignored_x_real_ip(&flag));
        }
    }

    #[test]
    fn max_socketio_connections_has_a_default_and_reads_the_environment() {
        assert_eq!(DEFAULT_MAX_SOCKETIO_CONNECTIONS, 20_000);
        assert_eq!(
            AppConfig::for_tests().max_socketio_connections,
            DEFAULT_MAX_SOCKETIO_CONNECTIONS
        );
        // SAFETY: `ENV_LOCK` is held for the whole body, so no other test
        // races this mutation of the process environment.
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe {
            std::env::set_var("MAX_SOCKETIO_CONNECTIONS", "2");
        }
        let config = AppConfig::from_env().expect("valid config");
        // SAFETY: see above.
        unsafe {
            std::env::remove_var("MAX_SOCKETIO_CONNECTIONS");
        }
        assert_eq!(config.max_socketio_connections, 2);
    }

    /// Acceptance criterion of listener limits task 002: the connect
    /// timeout is 10 seconds in the builder. `SocketIo::builder()`'s
    /// `connect_timeout` closes an Engine.IO session with no namespace
    /// connect (ADR 0007 §A Socket.IO Session With No Namespace), through a
    /// real `tokio::time::timeout` inside socketioxide. This checks the
    /// builder value `app(state)` gives it, instead of waiting 10 real
    /// seconds for that timer (AGENTS.md §7).
    #[tokio::test]
    async fn the_socketio_builder_sets_the_adr_0007_connect_timeout() {
        let state = RelayState::new(AppConfig::for_tests());
        let _router = app(state.clone());
        assert_eq!(state.socketio_connect_timeout(), Duration::from_secs(10));
    }

    #[test]
    fn artwork_max_bytes_has_a_default_and_reads_the_environment() {
        assert_eq!(DEFAULT_ARTWORK_MAX_BYTES, 524_288);
        assert_eq!(
            AppConfig::for_tests().artwork_max_bytes,
            DEFAULT_ARTWORK_MAX_BYTES
        );
        // SAFETY: `ENV_LOCK` is held for the whole body, so no other test
        // races this mutation of the process environment.
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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

    // ADR 0004, payment safety: the listener value and its Socket.IO emit
    // must never separate. `publish_metadata`, `expire_leases`,
    // `release_listener_updates`, and `delete_reserved` each take the
    // event's `listener_order` lock before they change the listener
    // timeline, and hold it through the emit.
    //
    // This test cannot observe the sequence of Socket.IO emits.
    // `emit_socket_remote_value` talks to a real `socketioxide::SocketIo`,
    // attached only by `app()`. Nothing in this crate records what it
    // sends, or in what order.
    //
    // So this test proves the lock structure directly instead. It holds
    // `listener_order` itself, the way a release sweep holds it across
    // its own emit. It shows that a concurrent publish cannot finish
    // until the lock is free. The other three call sites take the same
    // lock in the same order (`listener_order`, then `inner`), so the
    // same proof applies to each pair of them.
    #[tokio::test]
    async fn a_publish_waits_for_a_held_listener_order_lock() {
        let (state, _clock) = test_state(1);
        let created = state.create_event().await.expect("create event");
        let event = state.get_event(&created.event_id).await.expect("event");

        let guard = event.listener_order.lock().await;

        let publish_task = {
            let state = state.clone();
            let event_id = created.event_id.clone();
            let token = created.broadcaster_token.clone();
            tokio::spawn(async move {
                state
                    .publish_metadata(
                        &event_id,
                        &token,
                        Ok(0),
                        PublishMetadataRequest(json!({"title": "Live"})),
                    )
                    .await
            })
        };

        // Give the spawned task every chance to run. It must still be
        // waiting for `listener_order`.
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        assert!(
            !publish_task.is_finished(),
            "a publish must wait for a held listener_order lock"
        );

        drop(guard);
        let response = publish_task
            .await
            .expect("publish task joins")
            .expect("publish succeeds once listener_order is free");
        assert!(response.accepted);
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
