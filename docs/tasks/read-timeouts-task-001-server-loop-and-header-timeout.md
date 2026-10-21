# Read Timeouts Task 001: The Server Loop And The Header Timeout

Status: Implemented - 2026-10-21.

Every criterion is mechanical.

## Goal

A new server loop replaces `axum::serve`. It serves HTTP/1.1 only, with the
hyper header read timeout `HEADER_READ_TIMEOUT_SECS` (default 10). A
connection with no full request head, and an idle keep-alive connection,
close after that time. The handlers continue to get the TCP peer address,
WebSocket upgrades continue to operate, and the graceful shutdown stays
(ADR 0008 §Two Timeouts, §What The Timeouts Do Not Stop).

## Files To Inspect

- `docs/adr/0008-connection-read-timeouts.md`
- `docs/plans/adr-0008-read-timeouts-phase-plan.md`
- `src/main.rs`
- `src/lib.rs`: `AppConfig`, `AppConfig::from_env`, `AppConfig::for_tests`,
  `parse_lease_secs`, `ConfigError`, `app`, `connect_info_peer_addr`, the
  configuration unit tests near `ENV_LOCK`
- `tests/startup.rs`: how a test starts the relay binary
- `tests/api.rs`: how a test makes an event and reads an SSE stream
- `~/.cargo/registry/src/*/hyper-1.9.0/src/server/conn/http1.rs`
  (`Builder`, `with_upgrades`, `UpgradeableConnection::graceful_shutdown`)
- `~/.cargo/registry/src/*/axum-0.8.9/src/serve/mod.rs` (`run`,
  `handle_connection`: the shutdown pattern with two `watch` channels)
- `README.md`: the variable table and §Deployment

## Files Likely To Change

- `src/server.rs` (new)
- `src/lib.rs`
- `src/main.rs`
- `Cargo.toml`, `Cargo.lock`
- `tests/server.rs` (new)
- `README.md`

## Do Not Touch

- The routes, the handlers, the payloads, and the limits of ADR 0007.
- The client address rule in `src/lib.rs`. Change only the doc comment of
  `connect_info_peer_addr`, which names `axum::serve`.
- `BODY_READ_TIMEOUT_SECS` and the body deadline. Task 002 owns them.
- `docs/interoperability.md`, `docs/adr/**`, `AGENTS.md`, `docs/README.md`.

## Constraints

- Add to `[dependencies]` only:
  - `hyper = { version = "1", features = ["server", "http1"] }`
  - `hyper-util = { version = "0.1", features = ["service", "tokio"] }`
  - `tower = { version = "0.5", features = ["util"] }`

  Keep `tower` in `[dev-dependencies]` only if a duplicate entry is
  necessary.
- `src/server.rs` holds one public function, for example
  `pub async fn serve(listener: TcpListener, router: Router,
  header_read_timeout: Duration, shutdown: impl Future<Output = ()>) ->
  std::io::Result<()>`. Document it, with ADR 0008.
- For each accepted connection:
  - Insert `axum::extract::ConnectInfo(peer)` into the extensions of each
    request, for example with `tower::ServiceExt::map_request` on a clone of
    the router.
  - Use `hyper::server::conn::http1::Builder::new()` with
    `.timer(TokioTimer::new())` and
    `.header_read_timeout(header_read_timeout)`.
  - Serve with `.serve_connection(TokioIo::new(stream), service)
    .with_upgrades()`, with the service wrapped in
    `hyper_util::service::TowerToHyperService`.
  - Do not use `hyper_util::server::conn::auto` or
    `hyper_util::server::graceful`. The auto builder answers HTTP/2 with
    upgrades, and `GracefulShutdown` does not accept the upgradeable
    connection (correction of 2026-10-21).
  - Spawn each connection. Use the pattern of `handle_connection` in
    `axum::serve` 0.8.9: a `watch` channel tells each connection to call
    `graceful_shutdown()`, and a second `watch` channel lets the loop wait
    until each connection task ends.
- An accept error does not stop the loop. Log it with `tracing::warn!` and
  continue after a pause of 1 second.
- At the shutdown signal, stop the accept, tell each connection to stop,
  then wait for each connection task. Add no timeout to that wait. This is the
  behavior of `axum::serve` today.
- `main.rs` keeps `TraceLayer` and the shutdown signal. It gives
  `config.header_read_timeout` to the loop.
- `AppConfig` gets `header_read_timeout: Duration`. The default constant is
  `DEFAULT_HEADER_READ_TIMEOUT_SECS: u64 = 10`, with a doc comment that
  names ADR 0008. A value of `0` is a `ConfigError` that names
  `HEADER_READ_TIMEOUT_SECS`, as `parse_lease_secs` does for its minimum.
  `AppConfig::for_tests` uses the default.
- Do not use `axum::serve` in production code after this task.

## Implementation Steps

1. Add the dependencies.
2. Add the setting, its default, its parser and its unit tests.
3. Add `src/server.rs` and declare the module in `src/lib.rs`.
4. Change `src/main.rs` to call the loop.
5. Change the doc comment of `connect_info_peer_addr`.
6. Add `tests/server.rs` with the tests of §Acceptance Criteria. A helper
   binds `127.0.0.1:0`, starts the loop in a task with a header timeout of
   1 second, and gives the address. Use `std::net::TcpStream` or
   `tokio::net::TcpStream` with raw HTTP/1.1 bytes. Add no HTTP or
   WebSocket client dependency.
7. In `README.md`, add `HEADER_READ_TIMEOUT_SECS` to the variable table.
   In §Deployment, state that the relay serves HTTP/1.1 only. State that
   it closes a connection with no full request head after the timeout. The
   same timeout closes an idle connection.

## Acceptance Criteria

Each item is a test. The tests in `tests/server.rs` use a header timeout of
1 second and wait at most 5 seconds for one result.

- A request to `/health` gets `200`.
- A connection that sends `GET /health HTTP/1.1\r\nHost: x\r\n` and no
  more is closed by the relay (a read gives 0 bytes or a reset) before 5
  seconds. It is still open at 0.5 seconds.
- A keep-alive connection that is idle after one `200` is closed by the
  relay before 5 seconds.
- The handlers get the TCP peer address. With
  `MAX_LISTENER_CONNECTIONS_PER_CLIENT=1` and the default
  `TRUSTED_PROXIES`, an `/events` stream with `X-Real-IP: 203.0.113.1` and
  one with `X-Real-IP: 203.0.113.2` each get `200`. A second stream with
  `203.0.113.1` gets `429 listener_limit_reached`. Without the peer address,
  the peer is `0.0.0.0`, which is not trusted, and the second stream would
  get `429`.
- An `/events` stream stays open after 2.5 seconds: a publish after that
  time arrives on the stream.
- A WebSocket upgrade to `/socket.io/?EIO=4&transport=websocket` gets
  `101`. After 2.5 seconds, a read does not give end of stream.
- The bytes `PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n` do not get an HTTP/2 answer.
  The relay answers with `HTTP/1.1` or closes the connection.
- `HEADER_READ_TIMEOUT_SECS=0` and `HEADER_READ_TIMEOUT_SECS=x` are
  configuration errors that name the setting.
- Each existing test passes with no change to its assertions.

Also: the full gate passes.

## Test Commands

```bash
cargo fmt -- --check
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

## Escalation Triggers

- The hyper 1.9.0 HTTP/1 builder with `with_upgrades()` answers the
  HTTP/2 preface with HTTP/2.
- A test needs a new dependency other than the three in §Constraints.
- An existing test must change its assertion.
- The WebSocket test cannot get `101` with raw bytes.
- The idle keep-alive connection does not close. The spike says it does.

## Prompt for lower-context coding model

You are implementing one bounded task from a larger plan.

Implement only this task. Do not redesign the architecture.

Read:
- AGENTS.md
- docs/adr/0008-connection-read-timeouts.md
- docs/plans/adr-0008-read-timeouts-phase-plan.md
- docs/tasks/read-timeouts-task-001-server-loop-and-header-timeout.md
- src/main.rs, src/lib.rs (the items in §Files To Inspect), tests/startup.rs, tests/api.rs (event creation and SSE helpers), README.md
- the hyper 1.9.0 and axum 0.8.9 sources named in §Files To Inspect

Goal:
- Replace axum::serve with a server loop in src/server.rs.
- The loop uses hyper::server::conn::http1::Builder with with_upgrades(). It does not use the hyper-util auto builder.
- The loop serves HTTP/1.1 only, with HEADER_READ_TIMEOUT_SECS (default 10) as the hyper header read timeout.
- The loop inserts ConnectInfo<SocketAddr> into each request.
- The loop keeps WebSocket upgrades and the graceful shutdown.

Constraints:
- Follow §Constraints of the packet exactly. Only the dependencies listed there. No axum::serve in production code.
- Use the rust-dev skill conventions and the repository conventions in AGENTS.md.
- Write each comment and each document change in ASD-STE100 Simplified Technical English. Read /home/citizen/.agents/skills/asd-ste100/SKILL.md before you write prose, and run its checker on each changed document.

Do not touch:
- routes, handlers, payloads, ADR 0007 limits, the client address rule (only the doc comment of connect_info_peer_addr), BODY_READ_TIMEOUT_SECS, docs/interoperability.md, docs/adr/**, AGENTS.md, docs/README.md

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

Reviewed 2026-10-21. The full gate passes with 216 tests, and with 220
tests after the ADR 0009 packet.

- The first dispatch stopped at an escalation trigger. The auto builder of
  hyper-util answers the HTTP/2 preface with HTTP/2 when the caller uses
  `serve_connection_with_upgrades`, also after `http1_only()`. A probe got
  an HTTP/2 SETTINGS frame. The plan and this packet now use the HTTP/1
  builder of hyper. The test `the_http2_preface_does_not_get_an_http2_answer`
  guards the correction.
- The loop in `src/server.rs` follows the shutdown pattern of
  `axum::serve` 0.8.9 with two `watch` channels.
- The review found two defects and corrected them:
  - The loop logged each connection error at `warn`. Each slow client that
    the header timeout closes gives a connection error, so a client could
    fill the log. The loop now logs these errors at `debug`. `axum::serve`
    logged them at `trace`.
  - The loop paused for 1 second after each accept error. A reset or an
    aborted connection affects one connection only. The loop now accepts
    the next connection with no pause for these errors, as `axum::serve`
    did.
- A comment in `tests/api.rs` named `axum::serve` as the production path.
  It now names `server::serve`.
- The binary with `HEADER_READ_TIMEOUT_SECS=2` closed a slow header and an
  idle keep-alive connection after 2 seconds, with no warning line in the
  log.

