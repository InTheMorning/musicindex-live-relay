# Read Timeouts Task 002: The Body Deadline

Status: Implemented - 2026-10-21. It needs task 001.

Every criterion is mechanical.

## Goal

A request body that is not complete `BODY_READ_TIMEOUT_SECS` (default 30)
after the request head gets `408` with the error code
`request_body_timeout` and `Connection: close`. A slow flow of bytes does
not extend the deadline. A request that reads no body is not affected
(ADR 0008 §Two Timeouts, §What A Client Gets At A Timeout).

## Files To Inspect

- `docs/adr/0008-connection-read-timeouts.md`
- `docs/plans/adr-0008-read-timeouts-phase-plan.md`
- `src/lib.rs`: `AppConfig`, `app`, `ApiError`, `limit_socketio_handshake_rate`
  (a middleware example), `publish_metadata`, `publish_display`,
  `upload_artwork`, `reserve_event`
- `src/server.rs` and `tests/server.rs` from task 001
- `tests/api.rs`: the test app helpers and a publish test
- `~/.cargo/registry/src/*/tower-http-0.6.8/src/timeout/body.rs` (a body
  wrapper example. Its timeout starts again after each frame. Do not copy
  that behavior.)
- `README.md`: the variable table and the status code list
- `docs/interoperability.md`: §Limits That Consumers Work Around

## Files Likely To Change

- `src/lib.rs`
- `tests/api.rs`
- `tests/server.rs`
- `README.md`
- `docs/interoperability.md`

## Do Not Touch

- `src/server.rs`, except a change that a test of this task needs. Report
  each such change.
- The handlers and their own error codes. The middleware replaces their
  response only when the deadline ends.
- `RequestBodyLimitLayer` and each body limit.
- `Cargo.toml`, except one line in `[dev-dependencies]`:
  `tokio = { version = "1", features = ["test-util"] }`. The paused clock
  needs it (correction of 2026-10-21).
- `docs/adr/**`, `AGENTS.md`, `docs/README.md`.

## Constraints

- `AppConfig` gets `body_read_timeout: Duration`. The default constant is
  `DEFAULT_BODY_READ_TIMEOUT_SECS: u64 = 30`, with a doc comment that names
  ADR 0008. A value of `0` is a `ConfigError` that names
  `BODY_READ_TIMEOUT_SECS`.
- A body wrapper, for example `DeadlineBody`, holds the inner body, one
  pinned `tokio::time::Sleep` made when the request arrives, and a shared
  `Arc<AtomicBool>` flag.
  - In `poll_frame`, poll the sleep first. When the sleep is complete, set
    the flag and give an error. Do this before each frame, so that a slow
    flow of bytes cannot extend the deadline.
  - Else poll the inner body and give its result.
  - A body that is complete gives no error after the deadline.
  - Keep `size_hint` and `is_end_stream` of the inner body.
- A middleware wraps the body of each request and calls the next service.
  When the flag is set after the call, it gives
  `ApiError::new(StatusCode::REQUEST_TIMEOUT, "request_body_timeout")`
  with the header `Connection: close`. Else it gives the response of the
  next service.
- Apply the middleware in `app()` as the outermost layer, so that it also
  covers the Socket.IO layer. Read the timeout from the configuration in
  the state.
- Do not use `tower_http::timeout`.

## Implementation Steps

1. Add the setting, its default, its parser and its unit tests.
2. Add the body wrapper and the middleware, with doc comments that name
   ADR 0008.
3. Apply the middleware in `app()`.
4. Add the tests of §Acceptance Criteria.
5. In `README.md`, add `BODY_READ_TIMEOUT_SECS` to the variable table, and
   `408 request_body_timeout` to each route that reads a body.
6. In `docs/interoperability.md` §Limits That Consumers Work Around, add a
   section "A request body has a deadline". The section states these
   items:
   - the default of 30 seconds and the `408 request_body_timeout`,
   - a publisher on a slow uplink increases `BODY_READ_TIMEOUT_SECS`,
   - behind nginx with request buffering, the publisher does not get this
     `408`.

## Acceptance Criteria

Each item is a test.

In `tests/api.rs`, with `#[tokio::test(start_paused = true)]` and a body
timeout of 30 seconds:

- A metadata publish with a valid token and a body stream that gives one
  byte, then waits with no end, gets `408 request_body_timeout`. The
  response has `Connection: close`.
- A metadata publish with a valid token and a body stream that gives one
  byte every 10 seconds gets `408 request_body_timeout`. The paused clock
  shows 30 seconds or more, and less than 40 seconds, at the response.
- An artwork upload with no token and a body stream that waits with no end
  gets `408 request_body_timeout`.
- A metadata publish with a complete body gets `200`. The result stays
  `200` when the test moves the clock 60 seconds forward before it reads
  the response body.
- An `/events` stream that is open when the clock moves 60 seconds forward
  then gets a publish.

In `tests/server.rs`, through a real socket, with a body timeout of 1
second:

- `POST /v1/liveitems/{event_id}/metadata` with `Content-Length: 100` and
  one byte of body gets `HTTP/1.1 408` with `request_body_timeout` before 5
  seconds.

In `src/lib.rs`:

- `BODY_READ_TIMEOUT_SECS=0` and `BODY_READ_TIMEOUT_SECS=x` are
  configuration errors that name the setting.

Also: each existing test passes with no change to its assertions, and the
full gate passes.

## Test Commands

```bash
cargo fmt -- --check
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

## Escalation Triggers

- The middleware cannot see the flag after the call, for example because a
  layer moves the body to another task.
- A paused clock makes an existing helper in `tests/api.rs` fail.
- Socket.IO long-polling stops working under the middleware.
- An existing test must change its assertion.

## Prompt for lower-context coding model

You are implementing one bounded task from a larger plan.

Implement only this task. Do not redesign the architecture.

Read:
- AGENTS.md
- docs/adr/0008-connection-read-timeouts.md
- docs/plans/adr-0008-read-timeouts-phase-plan.md
- docs/tasks/read-timeouts-task-002-body-deadline.md
- src/lib.rs (the items in §Files To Inspect), src/server.rs, tests/server.rs, tests/api.rs, README.md, docs/interoperability.md

Goal:
- A request body that is not complete BODY_READ_TIMEOUT_SECS (default 30) after the request head gets 408 request_body_timeout with Connection: close. One deadline for the full body. Requests that read no body are not affected.

Constraints:
- Follow §Constraints of the packet exactly. Poll the deadline before each frame. Do not use tower_http::timeout. No new dependency.
- Use the rust-dev skill conventions and the repository conventions in AGENTS.md.
- Write each comment and each document change in ASD-STE100 Simplified Technical English. Read /home/citizen/.agents/skills/asd-ste100/SKILL.md before you write prose, and run its checker on each changed document.

Do not touch:
- src/server.rs (except a change a test needs, reported), the handlers and their error codes, RequestBodyLimitLayer and body limits, Cargo.toml, docs/adr/**, AGENTS.md, docs/README.md

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

Reviewed 2026-10-21. The full gate passes with 230 tests.

- The first dispatch stopped at an escalation trigger. A paused clock needs
  the tokio feature `test-util`, and the plan did not include it. The plan
  and this packet now permit `tokio` with `test-util` in
  `[dev-dependencies]`. `cargo tree -e normal,features -i tokio` shows no
  `test-util` in the normal build.
- `DeadlineBody` implements `hyper::body::Body`. That is the trait of
  `http-body`, which hyper exports. No new dependency was necessary.
- The first report of this packet claimed a test run that had not occurred.
  The agent corrected the claim in its next report. The review ran each
  test itself.
- The review added one guard and its test: a complete body gives no error
  when a caller polls it again after the deadline. The test
  `a_complete_body_polled_after_the_deadline_gives_no_error` fails without
  the guard.
- The review corrected one sentence of `docs/interoperability.md`. It
  stated request buffering on `api.musicindex.org` as a fact. Only the nginx
  example in `README.md` shows it.
- The binary with `HEADER_READ_TIMEOUT_SECS=2` and `BODY_READ_TIMEOUT_SECS=3`
  closed a slow header and an idle connection after 2 seconds. A slow body
  got `408 Request Timeout` after 3 seconds. The log had no warning line.

