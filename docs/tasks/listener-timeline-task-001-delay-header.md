# Listener Timeline Task 001: The Delay Header

Status: Ready - 2026-10-06. ADR 0004 is accepted.

Every criterion is mechanical.

## Goal

A publish can carry the header `Listener-Delay-Secs`. The relay validates it
and keeps the value of the last publish for each event. No timing changes in
this task.

## Files To Inspect

- `docs/adr/0004-listener-timeline-delay.md` (§The Delay, §Configuration)
- `src/lib.rs`: `AppConfig` and its `from_env`, the `DEFAULT_*` constants,
  `publish_metadata` (the method and the handler), `LiveEventState`
- `tests/api.rs` (the publish tests and the config tests)
- `README.md` (the publish route, its status codes, the variable table)

## Files Likely To Change

- `src/lib.rs`
- `tests/api.rs`
- `README.md`

## Do Not Touch

- The emit of Socket.IO, `remote_value`, `events`, `expire_leases`
- The two body forms and `into_metadata`
- `src/store.rs` and the SQLite schema
- `docs/adr/**`

## Constraints

- Add `DEFAULT_MAX_LISTENER_DELAY_SECS: u64 = 300` and
  `DEFAULT_MAX_PENDING_LISTENER_UPDATES: usize = 64`. Read them in
  `AppConfig::from_env` from `MAX_LISTENER_DELAY_SECS` and
  `MAX_PENDING_LISTENER_UPDATES`, with `env_parse`, as the other limits do.
- The handler reads the header. The header name does not depend on case.
- Rules for the value:
  - No header: the delay is 0.
  - A whole number from 0 to `MAX_LISTENER_DELAY_SECS`: that delay.
  - Any other value, also an empty value, a sign, a fraction, or more than one
    header: `400` with the code `invalid_listener_delay`.
- Keep the order of the present checks. Put the header check after the token
  check and before the rate-limit slot. A bad header then does not use a
  publish slot.
- Store the delay of the last accepted publish in `LiveEventState`, for
  example as an `AtomicU64`. Task 002 and task 003 read it.
- The publish response does not change.

## Implementation Steps

1. Add the two constants and the two `AppConfig` fields.
2. Read the raw header value in the handler, and pass it to
   `publish_metadata`. That method validates it after the token check.
3. Store the delay in `LiveEventState` after the publish is accepted.
4. Add the tests.
5. Update `README.md`: the header on the publish route, the `400` code, and
   the two variables in the table.

## Acceptance Criteria

Each item is a test in `tests/api.rs`:

- A publish with no header is accepted. The stored delay is 0.
- A publish with `Listener-Delay-Secs: 30` is accepted. The stored delay is
  30.
- `-1`, `1.5`, `abc`, an empty value and `301` each give
  `400 invalid_listener_delay`.
- A bad header does not use a publish slot of the rate limit.
- `MAX_LISTENER_DELAY_SECS=10` makes `11` give `400` and `10` accepted.
- A value of `MAX_PENDING_LISTENER_UPDATES` that is not a number is a
  configuration error.
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

- The publish handler cannot read headers without a change to other routes.
- A test outside this task fails.

## Prompt for lower-context coding model

You are implementing one bounded task from a larger plan.

Implement only this task. Do not redesign the architecture.

Read:
- docs/adr/0004-listener-timeline-delay.md
- docs/tasks/listener-timeline-task-001-delay-header.md
- src/lib.rs
- tests/api.rs
- README.md

Goal:
- Accept, validate and store the header `Listener-Delay-Secs` on a publish, and add the two limits. Change no timing.

Constraints:
- Follow §Constraints of the packet exactly.

Do not touch:
- Socket.IO emits, remote_value, events, expire_leases, the body forms, src/store.rs, docs/adr/**

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
