# ADR 0007 Listener Limits: Phase Plan

Status: Ready 2026-10-18. This plan does not make rules. ADR 0007 owns them.

## Goal

The relay limits three things: the open listener connections of each client
address, the new listener connections of each client address in one second,
and the open Socket.IO connections of the full relay. It finds the client
address with the rule of ADR 0007 §The Client Address.

## Non-Goals

- No limit for each client on publish or create. `MAX_CREATES_PER_SEC` stays
  global.
- No limit for each event.
- No change to the payload, the listener timeline, or the death modes.

## Assumptions

- `axum::serve` can give the TCP peer address through
  `into_make_service_with_connect_info::<SocketAddr>()`. The handlers then get
  `ConnectInfo<SocketAddr>` in the request extensions.
- `SocketRef::req_parts()` (socketioxide 0.18) gives the extensions of the
  handshake request, so a Socket.IO handler can read the same address.
- `SocketIoBuilder::connect_timeout` closes an Engine.IO session that does
  not connect to a namespace.
- The tests can operate Socket.IO with the Engine.IO polling transport through
  `tower::ServiceExt::oneshot`, and give the peer address with
  `axum::extract::connect_info::MockConnectInfo`. No new dependency is
  necessary.

## Affected Modules

| Module | Change |
|---|---|
| `src/main.rs` | Serve with connect info |
| `src/lib.rs` | `AppConfig` gets four settings. A client limiter keeps the counts for each client address. The SSE routes and the `/event` namespace use it. A global Socket.IO counter. The connect timeout. |
| `tests/api.rs` | Tests for each limit, each address rule and each status code |
| `README.md` | The four settings, `429 listener_limit_reached`, and the sentence about per-IP rate limiting |
| `AGENTS.md` §5 | The list of limits |

## Sequence

1. [Task 001](../tasks/listener-limits-task-001-client-address-and-sse.md):
   the client address, the client limiter, and the limits on the SSE routes.
2. [Task 002](../tasks/listener-limits-task-002-socket-io.md): the limits on
   Socket.IO, the global Socket.IO limit, and the connect timeout.

Task 002 needs task 001. Each task can ship by itself.

## Schema And API

- New settings: `TRUSTED_PROXIES`, `MAX_LISTENER_CONNECTIONS_PER_CLIENT`,
  `MAX_LISTENER_CONNECTS_PER_CLIENT_PER_SEC`, `MAX_SOCKETIO_CONNECTIONS`.
- New status: `429` with the error code `listener_limit_reached` on
  `/events` and `/display/events`.
- Socket.IO over a limit: `{}` and a disconnect, as for an unknown event.

## Risk Areas

- **Proxy trust.** If the relay does not trust nginx, each listener has the
  address `127.0.0.1`. The limit for each client then becomes a limit for the
  full relay. The default `TRUSTED_PROXIES` is `127.0.0.1,::1`. A test proves
  that a listener behind a trusted proxy gets its own count.
- **An incorrect address.** `X-Real-IP` from a peer that is not trusted must
  have no effect. A test proves it.
- **The count must go down.** Each closed connection decrements its count,
  also after an error or a disconnect by the client. A test proves it for
  SSE and for Socket.IO.
- **Memory for each client.** The limiter removes the entry of an address
  with no open connection and no recent connect.

## Test Strategy

Each limit, each address rule and each status code is a test in
`tests/api.rs`. Time comes from the injected time source of the relay, not
from a sleep (AGENTS.md §7).

## Rollback Strategy

Revert the commit. No stored data depends on it.

The review uses the
[ADR 0007 review checklist](../reviews/adr-0007-review-checklist.md).
