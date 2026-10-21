# ADR 0008 Read Timeouts: Review Checklist

Status: closed - 2026-10-21. Each item passes. This checklist is the named
artifact for the `Implemented` status of ADR 0008.

## Invariants Of ADR 0008

- [x] A connection with no full request head closes when
  `HEADER_READ_TIMEOUT_SECS` ends.
- [x] An idle keep-alive connection closes when `HEADER_READ_TIMEOUT_SECS`
  ends.
- [x] A request body that is not complete when `BODY_READ_TIMEOUT_SECS`
  ends gets `408 request_body_timeout`. A slow flow of bytes does not
  extend the deadline.
- [x] No timeout of ADR 0008 closes a listener stream or an upgraded
  connection.
- [x] Each timeout is configurable and has a default.
- [x] The relay serves HTTP/1.1 only (amendment of 2026-10-21).

## Code

- [x] No production code calls `axum::serve`.
- [x] Each request gets `ConnectInfo<SocketAddr>` from the new loop.
- [x] An accept error does not stop the loop.
- [x] The graceful shutdown stops the accept and waits for the open
  connections, as `axum::serve` did.
- [x] The body wrapper polls its deadline before each frame.
- [x] The body middleware is the outermost layer of `app()`.
- [x] `Cargo.toml` adds only `hyper`, `hyper-util` and `tower` to
  `[dependencies]`, and only `tokio` with `test-util` to
  `[dev-dependencies]`.
- [x] The loop uses the HTTP/1 builder of hyper, not the auto builder of
  hyper-util.
- [x] The tests in `tests/server.rs` use timeouts of 1 second and wait at
  most 5 seconds for one result.

## Documents

- [x] `README.md` documents both settings, `408 request_body_timeout`, and
  HTTP/1.1 only.
- [x] `docs/interoperability.md` has the section "A request body has a
  deadline".
- [x] `AGENTS.md` §5 lists the two timeouts.
- [x] ADR 0008 gets the status `Implemented`, with a dated sentence that
  names this checklist.

## Deployment

The operator does these checks on `api.musicindex.org` after the
deployment.

- [x] The three probes of ADR 0008 §Context against the relay port on the
  host each close after their timeout.
- [x] The publisher publishes metadata and uploads artwork with no `408`.
- [x] A Socket.IO app stays connected for 10 minutes.

## Deployment Result

The operator deployed commit `c22f647` on `api.musicindex.org` on
2026-10-21. The relay started at 20:09:46 UTC. The operator recorded these
results:

- The three probes against `http://127.0.0.1:8018` on the host:
  - The slow header closed after 10 seconds.
  - The idle keep-alive connection closed after 10 seconds.
  - The slow body with no token got `HTTP/1.1 408 Request Timeout` after 30
    seconds.
- The nginx access log has two `408` lines on a relay path. Both are the
  probes of ADR 0008 §Before Acceptance, at 13:25 (−0400), before the
  deployment. The log has no `408` on a path of the station event.
- After the deployment, the publisher sent artwork uploads, display
  publishes and a metadata publish at 17:16 and 17:18 (−0400). Each got
  `200`.
- A Socket.IO listener app stayed connected for more than 10 minutes and
  showed each new track and its artwork. The operator judged this visual
  item.

