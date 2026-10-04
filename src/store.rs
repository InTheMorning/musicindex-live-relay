//! The event store boundary (ADR 0001).
//!
//! The store holds the identity of each live item. The identity is the event
//! identifier, the token hash, the creation time, and the item class. It does
//! not hold the latest snapshot, the replay buffer, the broadcast sender, or
//! the lease time. That state is live state, and `RelayState` keeps it in
//! memory beside the store.

use std::collections::HashMap;

use subtle::ConstantTimeEq;

/// The class of a live item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventClass {
    /// The item lives in memory only. A restart or the idle TTL removes it.
    Ephemeral,
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
            .field("created_at", &self.created_at)
            .field("class", &self.class)
            .finish()
    }
}

/// The boundary for each read and write of a live item identity.
///
/// The caller holds the lock that protects the store. No method waits on a
/// subscriber.
pub trait EventStore: Send + Sync {
    /// Adds `event`. An item with the same identifier is replaced.
    fn insert(&mut self, event: StoredEvent);

    /// Returns the item with the identifier `event_id`, if it exists.
    fn get(&self, event_id: &str) -> Option<StoredEvent>;

    /// Removes the item with the identifier `event_id` and returns it.
    fn remove(&mut self, event_id: &str) -> Option<StoredEvent>;

    /// Returns the identifier of each item, in no specified order.
    fn list_ids(&self) -> Vec<String>;

    /// Keeps each item for which `keep` returns `true`, and removes the
    /// other items.
    ///
    /// Returns the identifier of each removed item.
    fn retain(&mut self, keep: &mut dyn FnMut(&StoredEvent) -> bool) -> Vec<String>;
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
    fn insert(&mut self, event: StoredEvent) {
        self.events.insert(event.event_id.clone(), event);
    }

    fn get(&self, event_id: &str) -> Option<StoredEvent> {
        self.events.get(event_id).cloned()
    }

    fn remove(&mut self, event_id: &str) -> Option<StoredEvent> {
        self.events.remove(event_id)
    }

    fn list_ids(&self) -> Vec<String> {
        self.events.keys().cloned().collect()
    }

    fn retain(&mut self, keep: &mut dyn FnMut(&StoredEvent) -> bool) -> Vec<String> {
        let mut removed = Vec::new();
        self.events.retain(|event_id, event| {
            let kept = keep(event);
            if !kept {
                removed.push(event_id.clone());
            }
            kept
        });
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(event_id: &str, created_at: u64) -> StoredEvent {
        StoredEvent {
            event_id: event_id.to_string(),
            token_hash: [7; 32],
            created_at,
            class: EventClass::Ephemeral,
        }
    }

    #[test]
    fn insert_then_get_returns_the_item() {
        let mut store = MemoryEventStore::new();
        store.insert(stored("a", 1));

        assert_eq!(store.get("a"), Some(stored("a", 1)));
        assert_eq!(store.get("missing"), None);
    }

    #[test]
    fn insert_with_the_same_identifier_replaces_the_item() {
        let mut store = MemoryEventStore::new();
        store.insert(stored("a", 1));
        store.insert(stored("a", 2));

        assert_eq!(store.get("a"), Some(stored("a", 2)));
        assert_eq!(store.list_ids(), vec!["a".to_string()]);
    }

    #[test]
    fn remove_returns_the_item_and_get_then_returns_none() {
        let mut store = MemoryEventStore::new();
        store.insert(stored("a", 1));

        assert_eq!(store.remove("a"), Some(stored("a", 1)));
        assert_eq!(store.get("a"), None);
        assert_eq!(store.remove("a"), None);
    }

    #[test]
    fn list_ids_returns_each_identifier() {
        let mut store = MemoryEventStore::new();
        assert!(store.list_ids().is_empty());

        store.insert(stored("a", 1));
        store.insert(stored("b", 2));

        let mut ids = store.list_ids();
        ids.sort();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn retain_removes_each_rejected_item_and_returns_its_identifier() {
        let mut store = MemoryEventStore::new();
        store.insert(stored("old", 1));
        store.insert(stored("new", 5));

        let removed = store.retain(&mut |event| event.created_at >= 3);

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
}
