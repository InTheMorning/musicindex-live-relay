# ADR 0007 Listener Limits: Review Checklist

Status: open - 2026-10-18. Tasks 001 and 002 are done, and each code and
document item passes. The deployment items are open. ADR 0007 becomes
`Implemented` only when each item passes.

## Invariants Of ADR 0007

- [x] Each listener connection counts against one client address.
- [x] The relay reads `X-Real-IP` only from a peer in `TRUSTED_PROXIES`, and
  never reads `X-Forwarded-For`.
- [x] A connection over a limit gets `{}` and a disconnect on Socket.IO, or
  `429` on SSE. It never gets a stale value.
- [x] Each limit is configurable and has a default.

## Code

- [x] Each closed connection releases its slots, for SSE and for Socket.IO.
- [x] A refused connection takes no slot.
- [x] The limiter removes the entry of an idle address.
- [x] No test sleeps for a timeout.
- [x] `Cargo.toml` has no new dependency.

## Documents

- [x] `README.md` documents the four settings, `429 listener_limit_reached`,
  the Socket.IO result at a limit, and the connect timeout.
- [x] The `README.md` sentence about per-IP rate limiting states ADR 0007 for
  listener connections.
- [x] `AGENTS.md` §5 lists the new limits.
- [x] `docs/interoperability.md` §A client has listener limits describes the
  limits as present.
- [ ] ADR 0007 gets the status `Implemented`, with a dated sentence that
  names this checklist.

## Deployment

A person must complete these items on `api.musicindex.org`. Report each one
as open until then.

- [ ] nginx sends `X-Real-IP $remote_addr` on `/socket.io/` and on
  `/v1/liveitems/`.
- [ ] After the deployment, network A opens 50 `/events` streams to one
  event, and stream 51 gets `429 listener_limit_reached`. At the same time,
  a stream from network B connects. This proves that the relay sees the
  address of each network, not `127.0.0.1`. The relay does not log client
  addresses, so this check uses the result.
