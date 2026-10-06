# ADR 0004 Listener Timeline: Phase Plan

Status: Ready - 2026-10-06. This plan does not make rules. ADR 0004 owns
them, and the operator accepted it on 2026-10-06.

## Goal

Socket.IO and `GET /remoteValue` give each live value update after the delay
that the broadcaster sends with the publish. SSE, `GET /metadata` and the
display routes stay instant.

## Non-Goals

- No change to the payload or to the two body forms.
- No storage of the delay. A restart needs no restore.
- No change to the display routes, which are instant already.
- No publisher change. `musicindex-live-publisher` ADR 0011 owns it.

## Assumptions

- The relay clock is the injected `Clock` of `RelayState::with_clock`. It
  counts whole seconds.
- The one-second sweep of `spawn_lease_task` can also release the pending
  updates. A delay is a whole number of seconds, so a sweep each second is
  sufficient.

## Affected Modules

| Module | Change |
|---|---|
| `src/lib.rs`, `AppConfig` | Two new limits with default constants |
| `src/lib.rs`, `publish_metadata` and its handler | Reads and validates `Listener-Delay-Secs` |
| `src/lib.rs`, `LiveEventState` | The pending list, the listener value and the last delay |
| `src/lib.rs`, `remote_value` and `register_socket_namespaces` | Read the listener value |
| `src/lib.rs`, `expire_leases`, `delete_reserved` | Put the expiry on the listener timeline, and remove pending updates on delete |
| `README.md`, `docs/interoperability.md` | The header, the new status code, the variables and the timing of each route |

## Sequence

1. [Task 001](../tasks/listener-timeline-task-001-delay-header.md): the header,
   its validation and the two limits. No timing changes.
2. [Task 002](../tasks/listener-timeline-task-002-listener-timeline.md): the
   pending list, the sweep, and the listener value for Socket.IO and
   `GET /remoteValue`.
3. [Task 003](../tasks/listener-timeline-task-003-expiry-delete-and-docs.md):
   the lease expiry and the delete on the listener timeline, the documents and
   the review.

Each task needs the task before it.

## Schema And API Implications

- New request header `Listener-Delay-Secs` on
  `POST /v1/liveitems/{event_id}/metadata`.
- New status: `400 invalid_listener_delay`.
- New variables: `MAX_LISTENER_DELAY_SECS` (default `300`) and
  `MAX_PENDING_LISTENER_UPDATES` (default `64`).
- No change to the SQLite schema.

## Risk Areas

- **The Socket.IO path of current podcast apps.** A publish with no header
  must behave exactly as before, with no added second of delay. Task 002
  emits at once when the delay is 0 and no update waits.
- **Lock order.** The sweep must not hold the event lock across a Socket.IO
  emit, the same rule that `expire_leases` follows.
- **The order rule.** A short delay after a long delay must wait.

## Test Strategy

Each rule has a test with the injected clock. No test uses a wall-clock sleep
(`AGENTS.md` §7). The tests call the sweep function directly, as the lease
tests call `expire_leases`.

## Rollback Strategy

A broadcaster that sends no header gets the behavior of today. To roll back,
deploy the relay version before task 002. The publisher of ADR 0011 must then
also roll back, or podcast apps get no delay.

## Review

[ADR 0004 review checklist](../reviews/adr-0004-review-checklist.md). ADR 0004
becomes `Implemented` after that review passes.
