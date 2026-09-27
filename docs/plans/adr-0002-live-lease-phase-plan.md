# ADR 0002 Live Lease Phase Plan

Date: 2026-09-27. This plan states no rule. ADR 0002 owns the rules here.

## Goal

An event goes off air when its broadcaster stops renewing it. A listener that
connects after that time never receives the last payee.

## Non-Goals

- A stop message from the broadcaster.
- A lease duration that the broadcaster chooses.
- A transition period for a broadcaster with no keepalive.
- Durable storage of the lease state.

## Assumptions

- No client uses the relay in production.
- `musicindex-live-publisher` is the only broadcaster. Its task 004 sends the
  keepalive after this plan is complete.
- `RelayState::with_clock` gives tests an injected clock with a resolution of
  one second.

## Affected Modules

| Module | Change | Task |
|---|---|---|
| `src/lib.rs` `AppConfig` | `LEASE_SECS` | 001 |
| `src/lib.rs` `LiveEventState` | `renewed_at` | 001 |
| `src/lib.rs` `publish_metadata` | Renew the lease, and add the response fields | 001 |
| `src/lib.rs` lease task | Expire leases each second | 001 |
| `src/lib.rs` router | The keepalive route | 002 |
| `src/lib.rs` `latest_metadata` | `renewed_at` and `lease_expires_at` | 002 |
| `README.md`, `docs/interoperability.md` | The contract | 002 |

## Sequence

1. The operator accepts ADR 0002.
2. Task 001: the lease state and the expiry.
3. Task 002: the keepalive route and the documents.
4. `musicindex-live-publisher` task 004 uses the route.

This work and ADR 0001 reserved items task 001 both change `LiveEventState`.
They can go in either order. The second one to land rebases on the first. The
reserved store boundary must keep `renewed_at` in memory only.

## Schema And API Implications

All wire changes add fields or a route. No field is renamed or removed.

- `POST /v1/liveitems/{event_id}/metadata` response: `lease_secs`,
  `keepalive_interval_secs`.
- `POST /v1/liveitems/{event_id}/keepalive`: new.
- `GET /v1/liveitems/{event_id}/metadata`: `renewed_at`, `lease_expires_at`.
  After an expiry it gives `404` with `metadata_not_found`.
- `remoteValue`, SSE and Socket.IO: `{}` after an expiry. That body already
  exists for an event with no snapshot.

## Risk Areas

- **The replay buffer.** A reconnect with `Last-Event-ID` must receive the
  `{}` update, not an older snapshot. Task 001 adds the `{}` update as a normal
  entry with its own `seq`.
- **A lock held across an await.** The expiry function takes the event write
  lock. Emit to SSE and Socket.IO after the lock is released, the same as a
  publish.
- **The idle TTL.** The lease and the idle TTL are separate. An expired lease
  does not remove the event.

## Test Strategy

- Unit tests with an injected clock call the expiry function directly. No test
  sleeps for a lease.
- `tests/api.rs` has one test for each status code of the keepalive route
  (AGENTS.md §7).
- A test proves that a keepalive after an expiry gives `409` and does not
  bring back the snapshot.

## Rollback

Each task is one commit. `git revert` restores the previous behavior. No task
stores data, so no rollback needs a data step. Revert task 002 before task 001.

## Tasks

- `docs/tasks/live-lease-task-001-lease-state-and-expiry.md`
- `docs/tasks/live-lease-task-002-keepalive-route-and-docs.md`
- `docs/reviews/live-lease-review-checklist.md`
