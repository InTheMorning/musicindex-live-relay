# Listener Limits Task 002: The Socket.IO Limits

Status: Implemented - 2026-10-18. It needs task 001.

Every criterion is mechanical.

## Goal

A Socket.IO connection to `/event` uses the client limiter of task 001. A
global counter limits the open Socket.IO connections of the full relay. An
Engine.IO session that does not connect to a namespace closes after 10
seconds.

## Files To Inspect

- `docs/adr/0007-listener-connection-limits.md` (§Three Limits, §What A
  Client Gets At A Limit, §A Socket.IO Session With No Namespace)
- `docs/research/curiohoster-livevalue-socketio-examples.md` (§Source: the
  Engine.IO polling sequence)
- `src/lib.rs`: the `SocketIo::builder()` call, `register_socket_namespaces`,
  `socket_event_id`, the client limiter of task 001
- `tests/api.rs`
- `README.md` (the Socket.IO section, the variable table)

## Files Likely To Change

- `src/lib.rs`
- `tests/api.rs`
- `README.md`
- `AGENTS.md` (§5, the list of limits)

## Do Not Touch

- The SSE limits and the client address rule of task 001.
- The value that a connection gets when it is accepted.
- The handling of an unknown event: `{}` and a disconnect.
- The publish, keepalive, create and reserve routes.
- `docs/adr/**`.

## Constraints

- Task 001 gives these functions in `src/lib.rs`. Use them, and do not copy
  their rules: `client_address` (the address rule), the `ClientPeerAddr`
  extractor (the TCP peer, or `0.0.0.0` with no `ConnectInfo`),
  `try_acquire_client_slot` and `release_client_slot` (the open count and
  the connect rate of each client), and `client_counter` (the amortized
  sweep). A Socket.IO connection uses the same slots as an SSE stream.
- `MAX_SOCKETIO_CONNECTIONS`, default `20000`.
- In the `/event` handler, the client address comes from
  `socket.req_parts()`, with the function of task 001.
- The handler checks the limits in this sequence: the global Socket.IO limit,
  then the open count of the client, then the event. A refused connection
  takes no slot.
- Above a limit: the handler sends `{}` on `remoteValue` and disconnects, as
  for an unknown event.
- An accepted connection keeps its slots until the socket disconnects. The
  disconnect handler releases them, also for a disconnect by the client.
- Each Engine.IO handshake counts as one new connection of its client for
  `MAX_LISTENER_CONNECTS_PER_CLIENT_PER_SEC`. A handshake is a request to
  `/socket.io/` with no `sid` query parameter. Above the limit, the relay
  answers `429 listener_limit_reached` to that request.
- `SocketIo::builder().connect_timeout(Duration::from_secs(10))`.
- The tests use the Engine.IO polling transport through `oneshot`, with
  `MockConnectInfo`. No new dependency.

## Implementation Steps

1. Add the setting and the global Socket.IO counter.
2. Use the client limiter and the global counter in the `/event` handler.
   Release the slots on disconnect.
3. Count each handshake against the connect rate.
4. Set the connect timeout.
5. Add the tests of §Acceptance Criteria.
6. Update `README.md`: the setting, the Socket.IO result at a limit, and the
   connect timeout. Update the list of limits in `AGENTS.md` §5.

## Acceptance Criteria

Each item is a test:

- From one address, the first 50 Socket.IO connections get the current
  value. Connection 51 gets `{}` and a disconnect.
- SSE streams and Socket.IO connections of one address share one count.
- After a disconnect by the client, its slot is free again.
- With `MAX_SOCKETIO_CONNECTIONS` set to 2, the third Socket.IO connection
  from a third address gets `{}` and a disconnect.
- Handshake 11 from one address in one second gets
  `429 listener_limit_reached`.
- A connection to an unknown event gets `{}` and a disconnect, as before,
  and takes no slot.
- The connect timeout is 10 seconds in the builder. This item is a unit test
  of the builder value, or a test that uses the time source of the relay. Do
  not sleep for 10 seconds.

Also: the full gate passes.

## Test Commands

```bash
cargo fmt -- --check
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

## Escalation Triggers

- The polling transport cannot run through `oneshot`.
- `socket.req_parts()` does not hold the `ConnectInfo` of the handshake.
- The disconnect handler cannot release a slot for each kind of disconnect.

## Prompt for lower-context coding model

You are implementing one bounded task from a larger plan.

Implement only this task. Do not redesign the architecture.

Read:
- docs/adr/0007-listener-connection-limits.md
- docs/plans/adr-0007-listener-limits-phase-plan.md
- docs/tasks/listener-limits-task-002-socket-io.md
- docs/research/curiohoster-livevalue-socketio-examples.md
- AGENTS.md
- src/lib.rs, tests/api.rs, README.md

Goal:
- A Socket.IO connection to /event uses the client limiter of task 001. A global counter limits the open Socket.IO connections. An Engine.IO session with no namespace closes after 10 seconds.

Constraints:
- Follow §Constraints of the packet exactly. No new dependency. Do not sleep for a timeout.

Do not touch:
- the SSE limits and the client address rule of task 001, the accepted connection value, the unknown-event handling, the publish, keepalive, create and reserve routes, docs/adr/**

Acceptance criteria:
- Each item in §Acceptance Criteria of the packet is a passing test.

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

Reviewed 2026-10-18. The full gate passes with 200 tests. The baseline at
`9c74809` had 176.

- The check sequence, the handshake count and the client address rule were
  correct at the first review. The handshake takes only the connect rate.
  The namespace connect takes only the open count. So one connection never
  counts two times.
- The first review found a race. `on_disconnect` was registered after
  `listener_value(...).await`. A disconnect during that await released no
  slot. The fix registers the handler directly after the slots are taken,
  and each later refusal only disconnects. `Socket::close` in socketioxide
  0.18.3 (`socket.rs`) takes the handler one time.
- The second review found a smaller window. socketioxide spawns the connect
  handler (`handler/connect.rs`, `tokio::spawn`) after it marks the socket
  connected. A disconnect before the handler runs finds no handler. The fix
  checks `socket.connected()` directly after the registration. A guard with
  an `AtomicBool` releases the slots one time for the two paths. `close()`
  sets the socket not connected before it takes the handler, so each order
  of events releases the slots.
- No test can control the order of two spawned tasks through `oneshot`. A
  unit test proves that the guard releases one time.
- `MAX_SOCKETIO_CONNECTIONS` has no value that turns it off, as for
  `MAX_SSE_CONNECTIONS`.

