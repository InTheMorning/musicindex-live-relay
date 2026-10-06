# Listener Timeline Task 003: Expiry, Delete And Documents

Status: Implemented - 2026-10-06, together with task 002.

Every criterion is mechanical. The visual check is in a separate list.

## Goal

A lease expiry reaches Socket.IO after the last block, with the delay of the
last publish. A delete removes the pending updates of the event. The documents
describe the two timelines.

## Files To Inspect

- `docs/adr/0004-listener-timeline-delay.md` (§The Order On The Listener
  Timeline, §The Lease And The `404`)
- `docs/adr/0002-live-lease.md` (§Expiry)
- `docs/tasks/listener-timeline-task-002-listener-timeline.md`
- `src/lib.rs`: `expire_leases`, `delete_reserved`,
  `disconnect_socket_clients`, the pending list of task 002
- `tests/api.rs`
- `docs/interoperability.md`, `README.md`, `AGENTS.md`

## Files Likely To Change

- `src/lib.rs`
- `tests/api.rs`
- `docs/interoperability.md`
- `README.md`
- `AGENTS.md`

## Do Not Touch

- The instant side of an expiry: the snapshot removal, `seq`, the replay
  buffer, the SSE send and `404 metadata_not_found` on `GET /metadata`
- The display routes and `clear_display_after_lease`
- `src/store.rs`
- `docs/adr/**`. The reviewer changes the status of ADR 0004.

## Constraints

- In `expire_leases`, replace the direct Socket.IO emit of `{}` with the
  publish rule of task 002. Use the stored delay of the last publish of the
  event. A delay of 0 with no pending update continues to emit at once.
- In `delete_reserved`, remove the pending updates of the event before
  `disconnect_socket_clients`. The delete still sends `{}` to the Socket.IO
  clients at once, as today.
- The release sweep never emits for an event that no longer exists.

## Implementation Steps

1. Change `expire_leases` as §Constraints says.
2. Change `delete_reserved` as §Constraints says.
3. Add the tests.
4. Update `docs/interoperability.md`: the route timing of ADR 0004 is no
   longer only proposed. Name the timing of each route.
5. Update `AGENTS.md` §Current State: Socket.IO and `GET /remoteValue` follow
   the delay of each publish, and SSE and the other reads are instant.

## Acceptance Criteria

Mechanical. Each item is a test with the injected clock:

- A publish with delay 30 at T, then a lease expiry at T + 90:
  `GET /metadata` gives `404` at T + 90. `GET /remoteValue` gives the payload
  until T + 119 and `{}` from T + 120.
- A delay longer than the lease: a publish with delay 120 at T, a second
  publish with delay 120 at T + 5, and the lease expiry at T + 95. The
  release order is the first payload at T + 120, the second at T + 125, and
  `{}` at T + 215.
- A publish with no header and a lease expiry: `GET /remoteValue` gives `{}`
  at the expiry, as before.
- A delete with a pending update: no release follows, and
  `release_listener_updates` gives 0 for that event.
- The existing lease tests and delete tests pass with no change.

Also: the full gate passes.

Visual. A person must examine this item. Report it as open until a person
completes the check.

- A podcast app on Socket.IO changes the block at the same time as before,
  with the same delay value from the broadcaster.

## Test Commands

```bash
cargo fmt -- --check
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

## Escalation Triggers

- An existing lease test asserts an immediate Socket.IO `{}` for an event
  that has a nonzero delay.
- The delete path cannot remove the pending updates without holding two
  locks at once.

## Prompt for lower-context coding model

You are implementing one bounded task from a larger plan.

Implement only this task. Do not redesign the architecture.

Read:
- docs/adr/0004-listener-timeline-delay.md
- docs/adr/0002-live-lease.md
- docs/tasks/listener-timeline-task-003-expiry-delete-and-docs.md
- src/lib.rs
- tests/api.rs
- docs/interoperability.md, README.md, AGENTS.md

Goal:
- Put the Socket.IO {} of a lease expiry on the listener timeline, remove pending updates on delete, and update the documents.

Constraints:
- Follow §Constraints of the packet exactly. Do not change the instant side of an expiry.

Do not touch:
- The snapshot removal, seq, the replay buffer, SSE, GET /metadata, the display routes, clear_display_after_lease, src/store.rs, docs/adr/**

Acceptance criteria:
- Each mechanical item in §Acceptance Criteria of the packet is a passing test with the injected clock.

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

## Review Result

Reviewed 2026-10-06 in one change with task 002. See the review result of
task 002. A lease expiry uses the publish rule with the delay of the last
publish. The test with a delay of 120 seconds and a lease of 90 seconds gives
the releases at T + 120, T + 125 and T + 215. A delete clears the pending
updates under `listener_order` before it disconnects the Socket.IO clients.

The visual check stays open: a podcast app on Socket.IO changes the block at
the same time as before.
