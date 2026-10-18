# ADR 0007 Listener Limits: Review Checklist

Status: open - 2026-10-18. No task is done. ADR 0007 becomes `Implemented`
only when each item passes.

## Invariants Of ADR 0007

- [ ] Each listener connection counts against one client address.
- [ ] The relay reads `X-Real-IP` only from a peer in `TRUSTED_PROXIES`, and
  never reads `X-Forwarded-For`.
- [ ] A connection over a limit gets `{}` and a disconnect on Socket.IO, or
  `429` on SSE. It never gets a stale value.
- [ ] Each limit is configurable and has a default.

## Code

- [ ] Each closed connection releases its slots, for SSE and for Socket.IO.
- [ ] A refused connection takes no slot.
- [ ] The limiter removes the entry of an idle address.
- [ ] No test sleeps for a timeout.
- [ ] `Cargo.toml` has no new dependency.

## Documents

- [ ] `README.md` documents the four settings, `429 listener_limit_reached`,
  the Socket.IO result at a limit, and the connect timeout.
- [ ] The `README.md` sentence about per-IP rate limiting states ADR 0007 for
  listener connections.
- [ ] `AGENTS.md` §5 lists the new limits.
- [ ] `docs/interoperability.md` §A client has listener limits says that ADR
  0007 is implemented.
- [ ] ADR 0007 gets the status `Implemented`, with a dated sentence that
  names this checklist.

## Deployment

A person must complete these items on `api.musicindex.org`. Report each one
as open until then.

- [ ] nginx sends `X-Real-IP $remote_addr` on `/socket.io/` and on
  `/v1/liveitems/`.
- [ ] After the deployment, two listeners from two networks connect to one
  event, and the relay log shows two client addresses, not `127.0.0.1`.
