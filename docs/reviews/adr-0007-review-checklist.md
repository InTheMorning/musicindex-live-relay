# ADR 0007 Listener Limits: Review Checklist

Status: closed - 2026-10-18. Each item passes. This checklist is the named
artifact for the `Implemented` status of ADR 0007.

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
- [x] ADR 0007 gets the status `Implemented`, with a dated sentence that
  names this checklist.

## Deployment

The operator completed these items on `api.musicindex.org` on 2026-10-18.

- [x] nginx sends `X-Real-IP $remote_addr` on `/socket.io/` and on
  `/v1/liveitems/`.
- [x] After the deployment, network A opens 50 `/events` streams to one
  event, and stream 51 gets `429 listener_limit_reached`. At the same time,
  a stream from network B connects. This proves that the relay sees the
  address of each network, not `127.0.0.1`. The relay does not log client
  addresses, so this check uses the result.

## Deployment Result

- `nginx -T` shows one `X-Real-IP $remote_addr` line in each of the
  locations `= /v1/liveitems`, `^~ /socket.io/` and `^~ /v1/liveitems/`.
- The first run of check 2 did not pass. Network B also got `429`. The relay runs
  in the Docker Compose service `live-relay` on the network
  `stophammer_default`. Docker forwards the nginx connection into the
  container from the bridge gateway `172.18.0.1`, not from `127.0.0.1`. That
  address was not in `TRUSTED_PROXIES`, so the relay used the gateway as the
  client address of each listener.
- The operator added `TRUSTED_PROXIES=127.0.0.1,::1,172.18.0.1` to
  `packaging/env/live-relay.compose.env` in the Stophammer deployment, and
  recreated the container. The second run passed. Network B got `200`.
- The gateway address belongs to the Compose network. If the network is
  made again with a different subnet, `TRUSTED_PROXIES` must change too.
  `README.md` §The Client Address states this.
