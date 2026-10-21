# ADR 0008 Read Timeouts: Phase Plan

Status: Done 2026-10-21. The packets and the deployment checks pass. This plan does not make rules. ADR 0008 owns them.

## Goal

A connection with no full request head closes after
`HEADER_READ_TIMEOUT_SECS`. An idle keep-alive connection closes after the
same time. A request body that is not complete after
`BODY_READ_TIMEOUT_SECS` gets `408 request_body_timeout`. The relay serves
HTTP/1.1 only. No timeout closes a listener stream or an upgraded
connection.

## Non-Goals

- No limit on the count of connections that are not listener connections.
- No token check before the body read.
- No timeout for a response, and no change for a slow SSE reader.
- No change of a route, a payload, or a limit of ADR 0007.

## Assumptions

- The spike of 2026-10-21 showed that the hyper 1.9.0 header read timeout,
  with `TokioTimer`, also closes an idle keep-alive connection.
- `hyper::server::conn::http1::Builder` with
  `.serve_connection(..).with_upgrades()` replaces `axum::serve`. This
  builder never reads the HTTP/2 preface, so it serves HTTP/1.1 only.
  Correction of 2026-10-21: the auto builder of hyper-util cannot do this.
  Its `http1_only()` has no effect with `serve_connection_with_upgrades`,
  and a probe got an HTTP/2 answer from it.
- The `GracefulShutdown` of hyper-util does not accept the upgradeable
  connection of hyper. The loop uses
  `UpgradeableConnection::graceful_shutdown` with two
  `tokio::sync::watch` channels, as `axum::serve` 0.8.9 does in
  `handle_connection`.
- The relay reads the peer address from the request extension
  `ConnectInfo<SocketAddr>` (`connect_info_peer_addr` in `src/lib.rs`). The
  new loop inserts that extension into each request.
- A body wrapper with one `tokio::time::Sleep` for each request gives a
  deadline for the full body. A test with
  `#[tokio::test(start_paused = true)]` can prove it with no wall-clock
  wait. Correction of 2026-10-21: a paused clock needs the tokio feature
  `test-util`. Task 002 adds it in `[dev-dependencies]` only, so the
  binary does not change.
- `hyper` 1.9.0, `hyper-util` 0.1.20 and `tower` 0.5 are already in
  `Cargo.lock`. Task 001 adds them to `[dependencies]` with the necessary
  features.

## Affected Modules

| Module | Change |
|---|---|
| `src/server.rs` (new) | The server loop: accept, the peer address, HTTP/1.1 with the header timeout, upgrades, and the graceful shutdown |
| `src/main.rs` | Calls the new loop in place of `axum::serve` |
| `src/lib.rs` | Two settings in `AppConfig`. The body deadline middleware in `app()`. The doc comment of `connect_info_peer_addr`. |
| `Cargo.toml` | `hyper`, `hyper-util` and `tower` in `[dependencies]`. `tokio` with `test-util` in `[dev-dependencies]` |
| `tests/server.rs` (new) | Tests with a real TCP listener |
| `tests/api.rs` | Tests of the body deadline with a paused clock |
| `README.md` | The two settings, `408 request_body_timeout`, HTTP/1.1 only |
| `docs/interoperability.md` | The body deadline and the `408` (task 002) |

## Sequence

1. [Task 001](../tasks/read-timeouts-task-001-server-loop-and-header-timeout.md):
   the server loop, HTTP/1.1 only, `HEADER_READ_TIMEOUT_SECS`.
2. [Task 002](../tasks/read-timeouts-task-002-body-deadline.md): the body
   deadline, `BODY_READ_TIMEOUT_SECS`, `408 request_body_timeout`, and the
   interoperability paragraph.

Task 002 needs task 001 only for its test through a real socket. Each task
can ship by itself.

## Schema And API

- New settings: `HEADER_READ_TIMEOUT_SECS` (default 10) and
  `BODY_READ_TIMEOUT_SECS` (default 30).
- New status: `408` with the error code `request_body_timeout`, on each
  route that reads a body, and with `Connection: close`.
- HTTP/2 with prior knowledge stops. nginx sends HTTP/1.1.

## Risk Areas

- **The client address.** If the new loop does not insert
  `ConnectInfo<SocketAddr>`, each client has the address `0.0.0.0`. The
  limits of ADR 0007 then apply to the full relay. A test in
  `tests/server.rs` proves that two `X-Real-IP` values from the trusted peer
  `127.0.0.1` count as two clients.
- **Upgrades.** A Socket.IO WebSocket needs `with_upgrades()`. A test proves that a WebSocket stays
  open after the header timeout.
- **The graceful shutdown.** `axum::serve` waits for each open connection
  after the signal. The new loop keeps that behavior. The review reads it.
- **A slow flow of bytes.** The body wrapper must examine its deadline
  before each frame, not only when the inner body waits. A test sends one
  byte in each half of the deadline.
- **Accept errors.** An accept error, for example too many open files, must
  not stop the loop. The loop logs it and continues after a short pause,
  as `axum::serve` does.

## Test Strategy

- `tests/server.rs` starts the relay on `127.0.0.1:0` with timeouts of 1
  second. Each test waits at most a few seconds. These tests measure the
  hyper timer through a real socket, so a paused clock cannot replace them.
  AGENTS.md §7 permits a short injected timeout.
- `tests/api.rs` proves the body deadline with
  `#[tokio::test(start_paused = true)]` and a body stream that waits.
- The configuration errors are unit tests in `src/lib.rs`, as for
  `LEASE_SECS`.

## Rollback Strategy

Revert the commits. No stored data depends on them. A revert gives back
`axum::serve`, with no timeouts.

The review uses the
[ADR 0008 review checklist](../reviews/adr-0008-review-checklist.md).
