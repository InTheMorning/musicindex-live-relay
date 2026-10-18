# Listener Limits Task 001: The Client Address And The SSE Limits

Status: Implemented - 2026-10-18.

Every criterion is mechanical.

## Goal

The relay finds the client address of each request with the rule of ADR 0007.
A client limiter counts the open listener connections and the new listener
connections of each client address. The SSE routes `/events` and
`/display/events` use it. Socket.IO does not change in this task.

## Files To Inspect

- `docs/adr/0007-listener-connection-limits.md` (§The Client Address,
  §Three Limits, §What A Client Gets At A Limit)
- `src/main.rs` (the `axum::serve` call)
- `src/lib.rs`: `AppConfig` and its `from_env`, the `DEFAULT_*` constants,
  `sse_connections`, `subscribe`, `subscribe_display`, the SSE handlers,
  `ApiError`, the time source of the relay
- `tests/api.rs` (the SSE tests, for example
  `max_sse_connections_enforced_and_decrements_on_drop` in `src/lib.rs`)
- `README.md` (the variable table, the SSE routes and their status codes, the
  sentence about per-IP rate limiting)

## Files Likely To Change

- `src/main.rs`
- `src/lib.rs`
- `tests/api.rs`
- `README.md`

## Do Not Touch

- Socket.IO: `register_socket_namespaces`, `socket_event_id`, the emit
  functions. Task 002 changes them.
- The publish, keepalive, create and reserve routes.
- The listener timeline of ADR 0004 and the lease of ADR 0002.
- `docs/adr/**`.

## Constraints

- `TRUSTED_PROXIES`: a comma-separated list of IP addresses. Default
  `127.0.0.1,::1`. A value that does not parse is a startup error that names
  the setting, as for the other settings.
- The client address: if the TCP peer is in `TRUSTED_PROXIES` and the request
  has one `X-Real-IP` header with one correct IP address, use that address.
  In all other conditions, use the TCP peer address. Never read
  `X-Forwarded-For`.
- When no `ConnectInfo` is in the request, the relay uses the address
  `0.0.0.0` for the client and logs this one time. This keeps a test that does
  not give a peer address working.
- `MAX_LISTENER_CONNECTIONS_PER_CLIENT`, default `50`.
  `MAX_LISTENER_CONNECTS_PER_CLIENT_PER_SEC`, default `10`. A value of `0`
  turns the limit off, as `try_acquire_window_slot` does for the other rate
  limits.
- The limiter keeps, for each client address, the open count and the connects
  in the current second. Use the time source of the relay.
- A connection takes its slot before `max_sse_connections`. A refused
  connection takes no slot of the two counters. A guard releases the slot
  when the stream ends, also after an error. Use the pattern of the current
  SSE counter.
- Above a limit for each client: `429` with the error code
  `listener_limit_reached`. The global limit keeps
  `503 max_sse_connections_reached`.
- The limiter removes the entry of an address when its open count is zero and
  its last connect is older than one second.
- Serve with `into_make_service_with_connect_info::<SocketAddr>()`.

## Implementation Steps

1. Add the three settings to `AppConfig`, with `DEFAULT_*` constants.
2. Add the client address function, and serve with connect info.
3. Add the client limiter, with its guard.
4. Use it in `/events` and `/display/events`.
5. Add the tests of §Acceptance Criteria. Give the peer address with
   `MockConnectInfo`.
6. In `README.md`, add the three settings, the `429` on the two SSE routes,
   and a short section on the client address. Change the sentence "Per-IP rate
   limiting is the responsibility of the front-end proxy" so that it states
   ADR 0007 for listener connections, and the proxy for publish and create.

## Acceptance Criteria

Each item is a test:

- From one address, connection 51 to `/events` gets
  `429 listener_limit_reached`. Connection 50 succeeds.
- The same for `/display/events`, and the two routes share one count.
- A second address is not affected by the limit of the first address.
- When a stream closes, its slot is free again.
- Connect 11 from one address in one second gets `429`. After the second
  ends, a connect succeeds. The test uses the time source of the relay.
- From a trusted peer, two values of `X-Real-IP` count as two clients.
- From a peer that is not trusted, `X-Real-IP` has no effect.
- `X-Forwarded-For` has no effect.
- A value of `0` turns each limit off.
- `TRUSTED_PROXIES` with a value that does not parse is a config error.
- The global `503 max_sse_connections_reached` test passes with no change.

Also: the full gate passes.

## Test Commands

```bash
cargo fmt -- --check
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

## Escalation Triggers

- `MockConnectInfo` cannot give the peer address to the routes in a test.
- A change to Socket.IO is necessary to pass a criterion.
- An existing test of a status code changes its result.

## Prompt for lower-context coding model

You are implementing one bounded task from a larger plan.

Implement only this task. Do not redesign the architecture.

Read:
- docs/adr/0007-listener-connection-limits.md
- docs/plans/adr-0007-listener-limits-phase-plan.md
- docs/tasks/listener-limits-task-001-client-address-and-sse.md
- AGENTS.md
- src/main.rs, src/lib.rs, tests/api.rs, README.md

Goal:
- The relay finds the client address with the rule of ADR 0007, counts the open and the new listener connections of each client address, and limits /events and /display/events. Socket.IO does not change.

Constraints:
- Follow §Constraints of the packet exactly. Never read X-Forwarded-For. Use the time source of the relay, not a sleep.

Do not touch:
- Socket.IO code, the publish, keepalive, create and reserve routes, the listener timeline, the lease, docs/adr/**

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

Reviewed 2026-10-18. The full gate passes with 190 tests. The baseline at
`9c74809` had 176.

- The address rule, the refusal rollback and the guards were correct at the
  first review. The tests use `MockConnectInfo` and the injected clock.
- The first review found that the map of client counters could grow without
  a bound. A connection that opened and closed in one second left its entry,
  because the prune at release keeps an entry with a connect in the current
  second. One client with many IPv6 addresses could grow the map.
- The fix has two parts. The TTL reaper calls `sweep_client_limits`, which
  runs one time each hour with the default TTL. `client_counter` sweeps the
  map when it reaches a threshold, and sets the next threshold to two times
  the size after the sweep, with a floor of 1024. The map thus holds the
  addresses with an open connection and the addresses of the current second,
  amortized. One shared function holds the removal rule.
- The review added one line: the hourly sweep also resets the threshold, so
  a flood does not leave a high threshold.

