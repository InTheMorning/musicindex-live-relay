# Live Lease Task 001: Lease State And Expiry

Status: Implemented - 2026-09-27. The review changed the lock order in
`expire_leases` and `keepalive`. The commit message gives the reason.

Every criterion is mechanical. This service has no user interface, so it has
no visual criteria.

## Goal

Each event has a lease that a publish renews. When the lease of an event with
a snapshot expires, the relay removes the snapshot and sends `{}` one time.
The publish response tells the broadcaster the lease duration and the
keepalive interval.

## Files To Inspect

- `docs/adr/0002-live-lease.md`
- `docs/plans/adr-0002-live-lease-phase-plan.md`
- `src/lib.rs` (`AppConfig`, `RelayState`, `with_clock`, `LiveEventState`,
  `publish_metadata`, `PublishMetadataResponse`, the replay buffer, the SSE and
  Socket.IO emit code, `spawn_cleanup_task`, `cleanup_expired`)
- `src/main.rs`
- `tests/api.rs`

## Files Likely To Change

- `src/lib.rs`
- `src/main.rs`
- `tests/api.rs`

## Do Not Touch

- `README.md` and `docs/interoperability.md`. Task 002 changes them.
- The router. Task 002 adds the route.
- The wrapped and direct body rule.
- The `remoteValue` body shape.

## Constraints

- `LEASE_SECS` comes from `AppConfig::from_env`. Add
  `DEFAULT_LEASE_SECS = 90`. A value less than 10 is a configuration error.
- The keepalive interval is `lease_secs / 3`, rounded down.
- `renewed_at` is in memory only.
- A publish sets `renewed_at` to `now`.
- The expiry function is `pub async fn expire_leases(&self) -> usize`. It uses
  `self.now()`, so tests control it with `with_clock`.
- For each event with a snapshot and `now - renewed_at >= lease_secs`:
  remove the snapshot, increment `seq`, add a `{}` update to the replay
  buffer, and emit `{}` to SSE and Socket.IO. Emit after the write lock is
  released.
- An event with no snapshot is not changed.
- An expiry does not remove the event and does not change `last_activity`.
- Add a background task that calls `expire_leases` each second. Start it next
  to `spawn_cleanup_task`.
- `PublishMetadataResponse` adds `lease_secs: u64` and
  `keepalive_interval_secs: u64`.

## Implementation Steps

1. Add `lease: Duration` to `AppConfig` with the default and the check.
2. Add `renewed_at: AtomicU64` to `LiveEventState`. Set it at creation and in
   `publish_metadata`.
3. Add `expire_leases` as the constraints state. Reuse the present emit code.
4. Add `spawn_lease_task` and start it in `main.rs`.
5. Add the two response fields.
6. Add tests with an injected clock:
   - a publish response has `lease_secs = 90` and `keepalive_interval_secs =
     30` with the default configuration,
   - before the lease duration, `expire_leases` returns 0 and `remoteValue`
     gives the snapshot,
   - at the lease duration, `expire_leases` returns 1, `remoteValue` gives
     `{}`, and `GET .../metadata` gives `404` with `metadata_not_found`,
   - after an expiry, an SSE reconnect with the last `seq` before the expiry
     receives `{}`,
   - a publish after an expiry brings the event back on air,
   - an event with no snapshot does not change,
   - `LEASE_SECS=5` is a configuration error.

## Acceptance Criteria

- Each test in step 6 exists and passes.
- No test sleeps for the lease duration.
- No lock is held across an emit.
- The idle TTL tests still pass without a change.

## Test Commands

```bash
cargo fmt -- --check
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

## Escalation Triggers

Stop and report if one of these occurs:

- The replay buffer cannot hold a `{}` update without a change to the SSE
  body shape.
- The clock resolution of one second makes a test unreliable.
- Reserved items task 001 landed first and changed `LiveEventState` in a way
  this packet does not name.

## Prompt for lower-context coding model

You are implementing one bounded task from a larger plan.

Implement only this task. Do not redesign the architecture.

Read:
- docs/adr/0002-live-lease.md
- docs/tasks/live-lease-task-001-lease-state-and-expiry.md
- src/lib.rs
- src/main.rs
- tests/api.rs

Goal:
- Each event has an in-memory lease that a publish renews. When the lease of an event with a snapshot expires, remove the snapshot, add a `{}` update to the replay buffer, and emit `{}` once. The publish response adds `lease_secs` and `keepalive_interval_secs`.

Constraints:
- `LEASE_SECS` from `AppConfig::from_env`, default 90, error below 10. Interval is lease_secs / 3, rounded down.
- `renewed_at` is in memory only. A publish sets it to `now`.
- `pub async fn expire_leases(&self) -> usize` uses `self.now()`.
- For an expired event with a snapshot: remove the snapshot, increment seq, add `{}` to the replay buffer, emit `{}` to SSE and Socket.IO after the write lock is released.
- An event with no snapshot does not change. An expiry does not remove the event or change last_activity.
- A background task calls `expire_leases` each second, started next to `spawn_cleanup_task`.

Do not touch:
- README.md and docs/interoperability.md
- The router
- The wrapped and direct body rule
- The remoteValue body shape

Acceptance criteria:
- Tests with an injected clock: default response values 90 and 30; no expiry before the lease duration; expiry at the lease duration gives remoteValue `{}` and metadata 404 metadata_not_found; an SSE reconnect after expiry receives `{}`; a publish after expiry brings the event back; an event with no snapshot does not change; LEASE_SECS=5 is an error.
- No test sleeps for the lease duration.
- No lock is held across an emit.
- The idle TTL tests pass without a change.

Test commands:
- cargo fmt -- --check
- cargo build
- cargo test
- cargo clippy --all-targets -- -D warnings

At the end, report:
1. files changed
2. tests run
3. behavior changed
4. deviations from task
5. unresolved concerns
