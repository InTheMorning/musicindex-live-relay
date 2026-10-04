//! The event store boundary (ADR 0001).
//!
//! The store holds the identity of each live item. The identity is the event
//! identifier, the token hash, the label, the creation time, and the item
//! class. It does not hold the latest snapshot, the replay buffer, the
//! broadcast sender, or the lease time. That state is live state, and
//! `RelayState` keeps it in memory beside the store.
//!
//! Two stores implement the boundary. `MemoryEventStore` keeps each item in
//! memory only. `SqliteEventStore` also writes each reserved item to a SQLite
//! file. Both stores answer each read from memory, so a read never touches
//! the disk.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use subtle::ConstantTimeEq;

/// The schema version that this build writes and reads.
pub const SCHEMA_VERSION: i64 = 1;

/// The schema of the state file. It runs in one transaction.
const SCHEMA: &str = "
CREATE TABLE schema_version (
    version INTEGER NOT NULL
) STRICT;
CREATE TABLE live_items (
    event_id   TEXT    NOT NULL PRIMARY KEY,
    token_hash BLOB    NOT NULL CHECK (length(token_hash) = 32),
    label      TEXT    UNIQUE,
    created_at INTEGER NOT NULL,
    class      TEXT    NOT NULL CHECK (class = 'reserved')
) STRICT;
";

/// One row of `live_items` as SQLite returns it, before validation: the
/// event identifier, the token hash, the label, the creation time, and the
/// class.
type RawRow = (String, Vec<u8>, Option<String>, i64, String);

/// The class of a live item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventClass {
    /// The item lives in memory only. A restart or the idle TTL removes it.
    Ephemeral,
    /// The item has a durable identity. `SqliteEventStore` writes it to the
    /// state file.
    Reserved,
}

impl EventClass {
    /// The value of the class column in the state file.
    fn as_column(self) -> &'static str {
        match self {
            Self::Ephemeral => "ephemeral",
            Self::Reserved => "reserved",
        }
    }

    /// The class for a value of the class column. The file holds reserved
    /// items only, so each other value is not valid.
    fn from_column(value: &str) -> Option<Self> {
        (value == Self::Reserved.as_column()).then_some(Self::Reserved)
    }
}

/// The stored identity of one live item.
///
/// The token hash is a SHA-256 hash. The store never holds the token.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredEvent {
    /// The event identifier.
    pub event_id: String,
    /// The SHA-256 hash of the broadcaster token.
    pub token_hash: [u8; 32],
    /// The operator name of a reserved item. An ephemeral item has none.
    pub label: Option<String>,
    /// The creation time, in seconds since the Unix epoch.
    pub created_at: u64,
    /// The class of the item.
    pub class: EventClass,
}

impl StoredEvent {
    /// Compares `candidate_hash` with the stored token hash in constant time.
    pub fn token_hash_matches(&self, candidate_hash: &[u8; 32]) -> bool {
        self.token_hash
            .as_slice()
            .ct_eq(candidate_hash.as_slice())
            .into()
    }
}

impl std::fmt::Debug for StoredEvent {
    // The token hash is not shown. A hash is not a token, but a log line does
    // not need it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredEvent")
            .field("event_id", &self.event_id)
            .field("token_hash", &"<redacted>")
            .field("label", &self.label)
            .field("created_at", &self.created_at)
            .field("class", &self.class)
            .finish()
    }
}

/// An error from a store.
///
/// The `Display` output holds no token and no token hash. A `Database` error
/// can hold internal detail, so a route must not send it to a client.
#[derive(Debug)]
pub enum StoreError {
    /// The store cannot open or prepare the state file at `path`.
    Open {
        /// The path of the state file.
        path: PathBuf,
        /// The cause.
        source: rusqlite::Error,
    },
    /// The state file at `path` has a schema version that this build does not
    /// know.
    UnsupportedSchema {
        /// The path of the state file.
        path: PathBuf,
        /// The version in the file, if the file holds one.
        found: Option<i64>,
    },
    /// The state file at `path` is corrupt, or a stored row is not valid.
    ///
    /// The relay does not start with this error. It does not change, delete
    /// or make again the file.
    Corrupt {
        /// The path of the state file.
        path: PathBuf,
        /// The cause, with no token hash.
        reason: String,
    },
    /// A read or a write of the state file failed.
    Database(rusqlite::Error),
    /// The lock on the database connection is poisoned.
    Poisoned,
    /// A reserved item already holds the label.
    LabelTaken,
    /// The store holds the maximum count of reserved items.
    ReservedLimitReached,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open { path, source } => {
                write!(f, "cannot open state file {}: {source}", path.display())
            }
            Self::UnsupportedSchema { path, found } => write!(
                f,
                "state file {} has schema version {found:?}, expected {SCHEMA_VERSION}",
                path.display()
            ),
            Self::Corrupt { path, reason } => {
                write!(f, "state file {} is corrupt: {reason}", path.display())
            }
            Self::Database(source) => write!(f, "state file error: {source}"),
            Self::Poisoned => f.write_str("state file connection lock is poisoned"),
            Self::LabelTaken => f.write_str("label is already reserved"),
            Self::ReservedLimitReached => f.write_str("maximum reserved item count reached"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Open { source, .. } | Self::Database(source) => Some(source),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(source: rusqlite::Error) -> Self {
        Self::Database(source)
    }
}

/// The boundary for each read and write of a live item identity.
///
/// The caller holds the lock that protects the store. No method waits on a
/// subscriber. A read comes from memory and cannot fail. A write can touch
/// the disk, so it returns a `Result`. A failed write changes nothing.
pub trait EventStore: Send + Sync {
    /// Adds `event`. An item with the same identifier is replaced.
    ///
    /// # Errors
    ///
    /// Returns `LabelTaken` when another reserved item holds the label,
    /// `ReservedLimitReached` when the store is full, and `Database` when the
    /// write fails.
    fn insert(&mut self, event: StoredEvent) -> Result<(), StoreError>;

    /// Returns the item with the identifier `event_id`, if it exists.
    fn get(&self, event_id: &str) -> Option<StoredEvent>;

    /// Removes the item with the identifier `event_id` and returns it.
    ///
    /// # Errors
    ///
    /// Returns `Database` when the write fails.
    fn remove(&mut self, event_id: &str) -> Result<Option<StoredEvent>, StoreError>;

    /// Returns the identifier of each item, in no specified order.
    fn list_ids(&self) -> Vec<String>;

    /// Keeps each item for which `keep` returns `true`, and removes the
    /// other items.
    ///
    /// Returns the identifier of each removed item.
    ///
    /// # Errors
    ///
    /// Returns `Database` when the write fails. Then no item is removed.
    fn retain(
        &mut self,
        keep: &mut dyn FnMut(&StoredEvent) -> bool,
    ) -> Result<Vec<String>, StoreError>;
}

/// An event store that keeps each item in memory only.
#[derive(Debug, Default)]
pub struct MemoryEventStore {
    events: HashMap<String, StoredEvent>,
}

impl MemoryEventStore {
    /// Makes an empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl EventStore for MemoryEventStore {
    fn insert(&mut self, event: StoredEvent) -> Result<(), StoreError> {
        self.events.insert(event.event_id.clone(), event);
        Ok(())
    }

    fn get(&self, event_id: &str) -> Option<StoredEvent> {
        self.events.get(event_id).cloned()
    }

    fn remove(&mut self, event_id: &str) -> Result<Option<StoredEvent>, StoreError> {
        Ok(self.events.remove(event_id))
    }

    fn list_ids(&self) -> Vec<String> {
        self.events.keys().cloned().collect()
    }

    fn retain(
        &mut self,
        keep: &mut dyn FnMut(&StoredEvent) -> bool,
    ) -> Result<Vec<String>, StoreError> {
        let mut removed = Vec::new();
        self.events.retain(|event_id, event| {
            let kept = keep(event);
            if !kept {
                removed.push(event_id.clone());
            }
            kept
        });
        Ok(removed)
    }
}

/// An event store that writes each reserved item to a SQLite file.
///
/// The store keeps every item in memory, and it answers each read from
/// memory. An ephemeral item never reaches the file. A reserved item reaches
/// the file in one transaction before the store changes its memory, so a
/// failed write leaves both sides unchanged.
///
/// The file holds identity and the token hash only. It never holds a
/// payload, a snapshot, a sequence number, or a replay buffer.
///
/// Each write is a blocking SQLite call. The caller must not make a write on
/// an async worker thread for a long time.
#[derive(Debug)]
pub struct SqliteEventStore {
    path: PathBuf,
    events: HashMap<String, StoredEvent>,
    // The trait needs `Sync`, and a `Connection` is not `Sync`. Each write
    // has `&mut self` and uses `Mutex::get_mut`, so the mutex never blocks.
    connection: Mutex<Connection>,
    max_reserved: usize,
}

impl SqliteEventStore {
    /// Opens the state file at `path`, or makes it when it does not exist.
    ///
    /// A new file gets the schema and the schema version in one transaction.
    /// The parent directory must exist. The store does not load the reserved
    /// rows of the file into memory. Call [`SqliteEventStore::load`] for that.
    ///
    /// # Errors
    ///
    /// Returns `Open` when SQLite cannot open or prepare the file, and
    /// `UnsupportedSchema` when the file holds a schema version that this
    /// build does not know.
    pub fn open(path: &Path, max_reserved: usize) -> Result<Self, StoreError> {
        let open_error = |source| StoreError::Open {
            path: path.to_path_buf(),
            source,
        };
        let mut connection = Connection::open(path).map_err(open_error)?;

        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(open_error)?;
        let has_version_table: bool = transaction
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM sqlite_schema \
                 WHERE type = 'table' AND name = 'schema_version')",
                [],
                |row| row.get(0),
            )
            .map_err(open_error)?;
        if has_version_table {
            let found: Option<i64> = transaction
                .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
                .optional()
                .map_err(open_error)?;
            if found != Some(SCHEMA_VERSION) {
                return Err(StoreError::UnsupportedSchema {
                    path: path.to_path_buf(),
                    found,
                });
            }
        } else {
            transaction.execute_batch(SCHEMA).map_err(open_error)?;
            transaction
                .execute(
                    "INSERT INTO schema_version (version) VALUES (?1)",
                    params![SCHEMA_VERSION],
                )
                .map_err(open_error)?;
        }
        transaction.commit().map_err(open_error)?;

        Ok(Self {
            path: path.to_path_buf(),
            events: HashMap::new(),
            connection: Mutex::new(connection),
            max_reserved,
        })
    }

    /// Reads each reserved row of the state file into memory, and returns
    /// the restored items (ADR 0001).
    ///
    /// A row holds identity and the token hash only. The restored items
    /// replace the items in memory. A new file gives an empty set.
    ///
    /// # Errors
    ///
    /// Returns `Corrupt` when the integrity check of SQLite fails or a row is
    /// not valid, and `Open` when SQLite cannot read the file. The error names
    /// the path. A failed load does not change the file or the memory.
    pub fn load(&mut self) -> Result<Vec<StoredEvent>, StoreError> {
        let path = self.path.clone();
        let read_error = |source| StoreError::Open {
            path: path.clone(),
            source,
        };
        let corrupt = |reason: String| StoreError::Corrupt {
            path: path.clone(),
            reason,
        };

        let connection = self.connection()?;
        let check: Vec<String> = connection
            .prepare("PRAGMA quick_check")
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| row.get(0))?
                    .collect::<Result<_, _>>()
            })
            .map_err(read_error)?;
        if check != ["ok"] {
            return Err(corrupt(format!(
                "integrity check failed: {}",
                check.join("; ")
            )));
        }

        let rows: Vec<RawRow> = connection
            .prepare("SELECT event_id, token_hash, label, created_at, class FROM live_items")
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    })?
                    .collect::<Result<_, _>>()
            })
            .map_err(read_error)?;

        let mut restored = Vec::with_capacity(rows.len());
        for (event_id, token_hash, label, created_at, class) in rows {
            let token_hash: [u8; 32] = token_hash
                .try_into()
                .map_err(|_| corrupt("a token hash does not have 32 bytes".to_string()))?;
            let created_at = u64::try_from(created_at)
                .map_err(|_| corrupt("a creation time is negative".to_string()))?;
            let class = EventClass::from_column(&class)
                .ok_or_else(|| corrupt("a row has an unknown item class".to_string()))?;
            restored.push(StoredEvent {
                event_id,
                token_hash,
                label,
                created_at,
                class,
            });
        }

        self.events = restored
            .iter()
            .map(|event| (event.event_id.clone(), event.clone()))
            .collect();
        Ok(restored)
    }

    fn connection(&mut self) -> Result<&mut Connection, StoreError> {
        self.connection.get_mut().map_err(|_| StoreError::Poisoned)
    }

    fn is_reserved(&self, event_id: &str) -> bool {
        self.events
            .get(event_id)
            .is_some_and(|event| event.class == EventClass::Reserved)
    }

    /// Writes one reserved row in one transaction.
    ///
    /// The file, not memory, decides the label check and the count check.
    /// The file can hold rows from an earlier process that memory does not
    /// hold.
    fn write_reserved(&mut self, event: &StoredEvent) -> Result<(), StoreError> {
        let max_reserved = self.max_reserved;
        let connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let others: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM live_items WHERE event_id <> ?1",
            params![event.event_id],
            |row| row.get(0),
        )?;
        if usize::try_from(others).unwrap_or(usize::MAX) >= max_reserved {
            return Err(StoreError::ReservedLimitReached);
        }

        if let Some(label) = &event.label {
            let taken: bool = transaction.query_row(
                "SELECT EXISTS (SELECT 1 FROM live_items WHERE label = ?1 AND event_id <> ?2)",
                params![label, event.event_id],
                |row| row.get(0),
            )?;
            if taken {
                return Err(StoreError::LabelTaken);
            }
        }

        let created_at = i64::try_from(event.created_at).unwrap_or(i64::MAX);
        transaction.execute(
            "INSERT INTO live_items (event_id, token_hash, label, created_at, class) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT (event_id) DO UPDATE SET \
             token_hash = excluded.token_hash, label = excluded.label, \
             created_at = excluded.created_at, class = excluded.class",
            params![
                event.event_id,
                event.token_hash.as_slice(),
                event.label,
                created_at,
                EventClass::Reserved.as_column(),
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Deletes the rows of `event_ids` in one transaction.
    fn delete_rows(&mut self, event_ids: &[&str]) -> Result<(), StoreError> {
        if event_ids.is_empty() {
            return Ok(());
        }
        let connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut statement =
                transaction.prepare("DELETE FROM live_items WHERE event_id = ?1")?;
            for event_id in event_ids {
                statement.execute(params![event_id])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }
}

impl EventStore for SqliteEventStore {
    fn insert(&mut self, event: StoredEvent) -> Result<(), StoreError> {
        match event.class {
            EventClass::Reserved => self.write_reserved(&event)?,
            // An ephemeral item never reaches the file. The file changes only
            // when the new item replaces a reserved item with the same
            // identifier.
            EventClass::Ephemeral => {
                if self.is_reserved(&event.event_id) {
                    self.delete_rows(&[event.event_id.as_str()])?;
                }
            }
        }
        self.events.insert(event.event_id.clone(), event);
        Ok(())
    }

    fn get(&self, event_id: &str) -> Option<StoredEvent> {
        self.events.get(event_id).cloned()
    }

    fn remove(&mut self, event_id: &str) -> Result<Option<StoredEvent>, StoreError> {
        if self.is_reserved(event_id) {
            self.delete_rows(&[event_id])?;
        }
        Ok(self.events.remove(event_id))
    }

    fn list_ids(&self) -> Vec<String> {
        self.events.keys().cloned().collect()
    }

    fn retain(
        &mut self,
        keep: &mut dyn FnMut(&StoredEvent) -> bool,
    ) -> Result<Vec<String>, StoreError> {
        let removed: Vec<String> = self
            .events
            .values()
            .filter(|event| !keep(event))
            .map(|event| event.event_id.clone())
            .collect();
        let reserved: Vec<&str> = removed
            .iter()
            .filter(|event_id| self.is_reserved(event_id))
            .map(String::as_str)
            .collect();

        // Write the file first. A failed write leaves memory unchanged.
        self.delete_rows(&reserved)?;
        for event_id in &removed {
            self.events.remove(event_id);
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type FileRow = (String, Vec<u8>, Option<String>, i64, String);

    fn stored(event_id: &str, created_at: u64) -> StoredEvent {
        StoredEvent {
            event_id: event_id.to_string(),
            token_hash: [7; 32],
            label: None,
            created_at,
            class: EventClass::Ephemeral,
        }
    }

    fn reserved(event_id: &str, label: Option<&str>) -> StoredEvent {
        StoredEvent {
            event_id: event_id.to_string(),
            token_hash: [9; 32],
            label: label.map(str::to_string),
            created_at: 5,
            class: EventClass::Reserved,
        }
    }

    fn file_rows(path: &Path) -> Vec<FileRow> {
        let connection = Connection::open(path).expect("open state file");
        let mut statement = connection
            .prepare(
                "SELECT event_id, token_hash, label, created_at, class \
                 FROM live_items ORDER BY event_id",
            )
            .expect("prepare");
        statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows")
    }

    fn open_store(max_reserved: usize) -> (tempfile::TempDir, PathBuf, SqliteEventStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.sqlite3");
        let store = SqliteEventStore::open(&path, max_reserved).expect("open");
        (dir, path, store)
    }

    #[test]
    fn insert_then_get_returns_the_item() {
        let mut store = MemoryEventStore::new();
        store.insert(stored("a", 1)).expect("insert");

        assert_eq!(store.get("a"), Some(stored("a", 1)));
        assert_eq!(store.get("missing"), None);
    }

    #[test]
    fn insert_with_the_same_identifier_replaces_the_item() {
        let mut store = MemoryEventStore::new();
        store.insert(stored("a", 1)).expect("insert");
        store.insert(stored("a", 2)).expect("insert");

        assert_eq!(store.get("a"), Some(stored("a", 2)));
        assert_eq!(store.list_ids(), vec!["a".to_string()]);
    }

    #[test]
    fn remove_returns_the_item_and_get_then_returns_none() {
        let mut store = MemoryEventStore::new();
        store.insert(stored("a", 1)).expect("insert");

        assert_eq!(store.remove("a").expect("remove"), Some(stored("a", 1)));
        assert_eq!(store.get("a"), None);
        assert_eq!(store.remove("a").expect("remove"), None);
    }

    #[test]
    fn list_ids_returns_each_identifier() {
        let mut store = MemoryEventStore::new();
        assert!(store.list_ids().is_empty());

        store.insert(stored("a", 1)).expect("insert");
        store.insert(stored("b", 2)).expect("insert");

        let mut ids = store.list_ids();
        ids.sort();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn retain_removes_each_rejected_item_and_returns_its_identifier() {
        let mut store = MemoryEventStore::new();
        store.insert(stored("old", 1)).expect("insert");
        store.insert(stored("new", 5)).expect("insert");

        let removed = store
            .retain(&mut |event| event.created_at >= 3)
            .expect("retain");

        assert_eq!(removed, vec!["old".to_string()]);
        assert_eq!(store.get("old"), None);
        assert_eq!(store.get("new"), Some(stored("new", 5)));
    }

    #[test]
    fn token_hash_matches_only_the_same_hash() {
        let event = stored("a", 1);

        assert!(event.token_hash_matches(&[7; 32]));
        assert!(!event.token_hash_matches(&[8; 32]));
    }

    #[test]
    fn debug_output_does_not_show_the_token_hash() {
        let output = format!("{:?}", stored("a", 1));

        assert!(output.contains("<redacted>"));
        assert!(!output.contains("[7, 7"));
    }

    #[test]
    fn sqlite_open_makes_the_schema_and_the_version() {
        let (_dir, path, store) = open_store(10);
        drop(store);

        let connection = Connection::open(&path).expect("open state file");
        let version: i64 = connection
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .expect("version");
        assert_eq!(version, SCHEMA_VERSION);

        // A second open of the same file keeps the schema.
        SqliteEventStore::open(&path, 10).expect("open again");
    }

    #[test]
    fn sqlite_open_rejects_an_unknown_schema_version() {
        let (_dir, path, store) = open_store(10);
        drop(store);
        Connection::open(&path)
            .expect("open state file")
            .execute("UPDATE schema_version SET version = 99", [])
            .expect("update");

        let err = SqliteEventStore::open(&path, 10).expect_err("unknown version");
        assert!(matches!(
            err,
            StoreError::UnsupportedSchema {
                found: Some(99),
                ..
            }
        ));
        assert!(err.to_string().contains(&path.display().to_string()));
    }

    #[test]
    fn sqlite_writes_a_reserved_item_and_not_an_ephemeral_item() {
        let (_dir, path, mut store) = open_store(10);

        store.insert(stored("e", 1)).expect("insert ephemeral");
        store
            .insert(reserved("r", Some("weekly show")))
            .expect("insert reserved");

        assert_eq!(store.get("e"), Some(stored("e", 1)));
        assert_eq!(store.get("r"), Some(reserved("r", Some("weekly show"))));
        assert_eq!(
            file_rows(&path),
            vec![(
                "r".to_string(),
                vec![9; 32],
                Some("weekly show".to_string()),
                5,
                "reserved".to_string(),
            )]
        );
    }

    #[test]
    fn sqlite_rejects_a_duplicate_label_and_changes_nothing() {
        let (_dir, path, mut store) = open_store(10);
        store.insert(reserved("a", Some("show"))).expect("insert");

        let err = store
            .insert(reserved("b", Some("show")))
            .expect_err("duplicate label");

        assert!(matches!(err, StoreError::LabelTaken));
        assert_eq!(store.get("b"), None);
        assert_eq!(file_rows(&path).len(), 1);
    }

    #[test]
    fn sqlite_accepts_more_than_one_item_with_no_label() {
        let (_dir, path, mut store) = open_store(10);

        store.insert(reserved("a", None)).expect("insert");
        store.insert(reserved("b", None)).expect("insert");

        assert_eq!(file_rows(&path).len(), 2);
    }

    #[test]
    fn sqlite_rejects_a_reserved_item_over_the_limit() {
        let (_dir, _path, mut store) = open_store(1);
        store.insert(reserved("a", None)).expect("insert");

        let err = store.insert(reserved("b", None)).expect_err("over limit");

        assert!(matches!(err, StoreError::ReservedLimitReached));
        assert_eq!(store.get("b"), None);
        // An ephemeral item does not count against the limit.
        store.insert(stored("e", 1)).expect("insert ephemeral");
    }

    #[test]
    fn sqlite_checks_the_label_against_rows_from_an_earlier_process() {
        let (_dir, path, mut store) = open_store(10);
        store.insert(reserved("a", Some("show"))).expect("insert");
        drop(store);

        let mut store = SqliteEventStore::open(&path, 10).expect("open again");
        let err = store
            .insert(reserved("b", Some("show")))
            .expect_err("label from the earlier process");

        assert!(matches!(err, StoreError::LabelTaken));
    }

    #[test]
    fn sqlite_remove_deletes_the_row() {
        let (_dir, path, mut store) = open_store(10);
        store.insert(reserved("r", None)).expect("insert");

        assert_eq!(
            store.remove("r").expect("remove"),
            Some(reserved("r", None))
        );
        assert_eq!(store.get("r"), None);
        assert!(file_rows(&path).is_empty());
    }

    #[test]
    fn sqlite_retain_deletes_each_removed_reserved_row() {
        let (_dir, path, mut store) = open_store(10);
        store.insert(stored("e", 1)).expect("insert");
        store.insert(reserved("r1", None)).expect("insert");
        store.insert(reserved("r2", None)).expect("insert");

        let mut removed = store
            .retain(&mut |event| event.event_id == "r2")
            .expect("retain");
        removed.sort();

        assert_eq!(removed, vec!["e".to_string(), "r1".to_string()]);
        assert_eq!(store.list_ids(), vec!["r2".to_string()]);
        let rows = file_rows(&path);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "r2");
    }

    #[test]
    fn sqlite_load_of_a_new_file_gives_an_empty_set() {
        let (_dir, _path, mut store) = open_store(10);

        assert!(store.load().expect("load").is_empty());
        assert!(store.list_ids().is_empty());
    }

    #[test]
    fn sqlite_load_restores_the_rows_of_an_earlier_process() {
        let (_dir, path, mut store) = open_store(10);
        store.insert(stored("e", 1)).expect("insert ephemeral");
        store
            .insert(reserved("r", Some("weekly show")))
            .expect("insert reserved");
        drop(store);

        let mut store = SqliteEventStore::open(&path, 10).expect("open again");
        let restored = store.load().expect("load");

        assert_eq!(restored, vec![reserved("r", Some("weekly show"))]);
        assert_eq!(store.get("r"), Some(reserved("r", Some("weekly show"))));
        // An ephemeral item never reached the file, so it is not restored.
        assert_eq!(store.get("e"), None);
        assert!(store.get("r").expect("r").token_hash_matches(&[9; 32]));
    }

    #[test]
    fn sqlite_load_rejects_a_row_that_is_not_valid() {
        let (_dir, path, store) = open_store(10);
        drop(store);
        let connection = Connection::open(&path).expect("open state file");
        connection
            .execute_batch("PRAGMA ignore_check_constraints = ON")
            .expect("pragma");
        connection
            .execute(
                "INSERT INTO live_items (event_id, token_hash, label, created_at, class) \
                 VALUES ('x', ?1, NULL, 1, 'ephemeral')",
                params![[9_u8; 32].as_slice()],
            )
            .expect("insert");
        drop(connection);

        let mut store = SqliteEventStore::open(&path, 10).expect("open");
        let err = store.load().expect_err("row with an unknown class");

        assert!(matches!(err, StoreError::Corrupt { .. }));
        assert!(err.to_string().contains(&path.display().to_string()));
        assert!(store.list_ids().is_empty());
    }

    #[test]
    fn sqlite_open_of_a_file_that_is_not_a_database_fails_and_keeps_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("state.sqlite3");
        let garbage = b"this is not a SQLite database, it is garbage text \x00\x01".repeat(64);
        std::fs::write(&path, &garbage).expect("write garbage");

        let err = SqliteEventStore::open(&path, 10)
            .and_then(|mut store| store.load().map(|_| store))
            .expect_err("garbage file");

        assert!(err.to_string().contains(&path.display().to_string()));
        assert_eq!(std::fs::read(&path).expect("read"), garbage);
    }
}
