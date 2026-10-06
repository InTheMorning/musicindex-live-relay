# Listener Timeline Task 002: The Listener Timeline

Status: Ready after task 001.

Every criterion is mechanical.

## Goal

Socket.IO and `GET /remoteValue` give each published update after the delay
of its publish. SSE and `GET /metadata` stay instant. A publish with no
header behaves exactly as before.

## Files To Inspect

- `docs/adr/0004-listener-timeline-delay.md` (§Two Timelines, §The Order On
  The Listener Timeline, §The Pending Updates)
- `docs/tasks/listener-timeline-task-001-delay-header.md`
- `src/lib.rs`: `LiveEventState`, `publish_metadata`,
  `emit_socket_remote_value`, `remote_value`, `register_socket_namespaces`,
  `expire_leases`, `spawn_lease_task`, `RelayState::now`
- `tests/api.rs` (the lease tests that drive the clock and call
  `expire_leases`)

## Files Likely To Change

- `src/lib.rs`
- `tests/api.rs`
- `README.md`

## Do Not Touch

- `events` (SSE), `latest_metadata`, the replay buffer, the display routes
- `expire_leases` and `delete_reserved`. Task 003 changes them.
- `src/store.rs`
- `docs/adr/**`

## Constraints

- Add to `LiveEventState`:
  - the listener value: the `Value` that Socket.IO and `GET /remoteValue`
    give now. It starts as `{}`.
  - the pending list: updates with a release time, oldest first.
  - the release time of the newest pending update, for the order rule.
- In `publish_metadata`, after the snapshot is stored and sent on SSE:
  - If the delay is 0 and the pending list is empty: set the listener value
    and emit on Socket.IO at once, as today.
  - Else: compute the release time as `now + delay`. If it is earlier than
    the release time of the newest pending update, use that time. Add the
    update to the pending list.
  - If the list then holds more than `MAX_PENDING_LISTENER_UPDATES`, the new
    update replaces the newest pending update. It keeps the later release
    time of the two.
- Add `RelayState::release_listener_updates(&self) -> usize`. For each event,
  it moves each pending update whose release time is not after `now` to the
  listener value, in order. It emits each one on Socket.IO. It returns the
  count of released updates.
- Follow the lock rule of `expire_leases`: copy the candidates, take one
  event lock at a time, and release the lock before each emit.
- Call `release_listener_updates` in the loop of `spawn_lease_task`, each
  second, after `expire_leases`.
- `remote_value` and the initial emit in `register_socket_namespaces` read the
  listener value, not `latest`.
- `GET /metadata` and SSE still read `latest` and the replay buffer.

## Implementation Steps

1. Add the three fields and their initial values.
2. Change `publish_metadata` as §Constraints says.
3. Add `release_listener_updates` and call it from `spawn_lease_task`.
4. Change `remote_value` and the Socket.IO connect handler.
5. Add the tests.
6. In `README.md`, write for each route whether it waits for the delay. Use
   the route table of ADR 0004 §Two Timelines. Add one sentence for client
   authors: only a client that waits for an in-band key may pay from SSE.

## Acceptance Criteria

Each item is a test with the injected clock. No test sleeps.

- A publish with no header: `GET /remoteValue` gives the new payload at once,
  with no call to `release_listener_updates`.
- A publish with delay 30 at time T: SSE and `GET /metadata` give it at once.
  `GET /remoteValue` gives the previous value at T + 29 and the new payload
  at T + 30, after `release_listener_updates`.
- A publish with delay 30 at T, then a publish with delay 0 at T + 1: the
  second update is released after the first, at T + 30, not at T + 1.
- A publish with delay 0 after a release of all pending updates is at once
  again.
- With `MAX_PENDING_LISTENER_UPDATES=2`, three publishes with delay 30 leave
  two pending updates. The third replaces the second. After the release,
  `GET /remoteValue` gives the third payload.
- A Socket.IO client that connects while an update waits gets the listener
  value, not the newest payload. If Socket.IO has no test harness in
  `tests/api.rs`, test the value that the connect handler reads through the
  same function that it calls.
- The existing tests pass with no change.

Also: the full gate passes.

## Test Commands

```bash
cargo fmt -- --check
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

## Escalation Triggers

- The connect handler cannot read the listener value without a lock order
  that differs from §Constraints.
- An existing test expects `GET /remoteValue` to read `latest` in a way that
  the listener value cannot satisfy.
- The release sweep cannot run inside `spawn_lease_task` without a change to
  its interval.

## Prompt for lower-context coding model

You are implementing one bounded task from a larger plan.

Implement only this task. Do not redesign the architecture.

Read:
- docs/adr/0004-listener-timeline-delay.md
- docs/tasks/listener-timeline-task-002-listener-timeline.md
- src/lib.rs
- tests/api.rs
- README.md

Goal:
- Give Socket.IO and GET /remoteValue a listener value that follows the delay of each publish. Keep SSE and GET /metadata instant. A publish with no header behaves as before.

Constraints:
- Follow §Constraints of the packet exactly, including the lock rule and the order rule.

Do not touch:
- events (SSE), latest_metadata, the replay buffer, the display routes, expire_leases, delete_reserved, src/store.rs, docs/adr/**

Acceptance criteria:
- Each item in §Acceptance Criteria of the packet is a passing test with the injected clock.

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
