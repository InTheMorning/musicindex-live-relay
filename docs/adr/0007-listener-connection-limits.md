# ADR 0007: Listener Connection Limits For Each Client

## Status

Proposed 2026-10-18.

Class: durable. AGENTS.md §5 requires a configurable limit with a default for
each resource. This record adds the limits for listener connections.

## Context

A listener connection is a long-lived connection that receives live data:

- a Socket.IO connection to the namespace `/event`,
- an SSE stream on `/v1/liveitems/{event_id}/events`,
- an SSE stream on `/v1/liveitems/{event_id}/display/events`.

The relay limits SSE streams with `MAX_SSE_CONNECTIONS` (default 1000) for the
full relay. It has no limit for Socket.IO connections. `SocketIo::builder()`
in `src/lib.rs` sets no limit. AGENTS.md §5 requires one.

A relay connection to an unknown event ends immediately, so an unknown
event identifier costs nothing. But each event identifier is public: it is in the
`podcast:liveValue` URI of an RSS feed. One client can open many connections
to a known event.

A global limit cannot keep a popular event available and also stop one
client:

- A low global limit turns away the listeners of a popular event.
- A high global limit lets one client use all of it.

`README.md` has this sentence: "Per-IP rate limiting is the responsibility of
the front-end proxy (e.g. nginx)." No ADR owns that sentence. The operator decided on
2026-10-18 that the relay limits listener connections for each client, so
that the limit is in the tested contract of the relay.

On `api.musicindex.org`, nginx proxies `/socket.io/` and `/v1/liveitems/` to
`127.0.0.1:8018`. It sets `X-Real-IP $remote_addr`. nginx always sets that
header, so a client cannot change it. Without proxy trust, the relay sees
each listener as `127.0.0.1`.

The research of a curiohoster show on 2026-10-17 found that curiohoster has
no connection limit and makes a room for each new identifier. The note is in
`~/build/musicindex-research/reports/`.

## Decision

### The Client Address

- `TRUSTED_PROXIES` is a list of IP addresses. The default is
  `127.0.0.1,::1`.
- A request can come from a TCP peer in `TRUSTED_PROXIES`, with `X-Real-IP`
  that holds one correct IP address. Then the client address is that address.
- In all other conditions, the client address is the TCP peer address.
- The relay never reads `X-Forwarded-For`.

The default trusts loopback because the default `BIND` is `127.0.0.1:8018`
behind a proxy. With no trust, a limit for each client would be a limit for
the full relay.

### Three Limits

| Setting | Default | Limit |
|---|---|---|
| `MAX_LISTENER_CONNECTIONS_PER_CLIENT` | `50` | Open listener connections from one client address. Socket.IO and SSE together. |
| `MAX_LISTENER_CONNECTS_PER_CLIENT_PER_SEC` | `10` | New listener connections from one client address in one second. |
| `MAX_SOCKETIO_CONNECTIONS` | `20000` | Open Socket.IO connections for the full relay. |

- `MAX_SSE_CONNECTIONS` stays, with its default.
- The limit for each client protects against one client. The global limits
  protect the memory of the relay.
- The default of 50 for each client permits a venue or a mobile network in
  which many listeners share one address. An operator can increase it.
- The relay removes the counter of a client address when it has no open
  connection and no recent connect.

### What A Client Gets At A Limit

- **Socket.IO:** the relay sends `{}` on `remoteValue`, then disconnects the
  client, as for an unknown event. The client does not reconnect. An app then
  uses the static `<podcast:value>` of the live item.
- **SSE:** `429` with the error code `listener_limit_reached` for a limit of a
  client. The global limit keeps `503 max_sse_connections_reached`.

### A Socket.IO Session With No Namespace

A client can open an Engine.IO session and never connect to `/event`. The
relay closes such a session after 10 seconds. The handshake counts against
`MAX_LISTENER_CONNECTS_PER_CLIENT_PER_SEC`.

## Invariants

- Each listener connection counts against one client address.
- The relay reads `X-Real-IP` only from a peer in `TRUSTED_PROXIES`.
- A connection over a limit gets `{}` and a disconnect on Socket.IO, or `429`
  on SSE. It never gets a stale value.
- Each limit is configurable and has a default.

## Non-Goals

- A limit for each client on publish and create. The proxy keeps that role.
  `MAX_CREATES_PER_SEC` stays global.
- A limit for each event.
- A change of the payload or of the listener timeline (ADR 0004).

## Before Acceptance

- The operator confirms the defaults: 50, 10 and 20000.
- `docs/interoperability.md` gets the new limits and the new `429`, because
  a consumer can see them.

## Verification After Implementation

- A test opens `MAX_LISTENER_CONNECTIONS_PER_CLIENT` Socket.IO connections
  from one address, and the next one gets `{}` and a disconnect.
- A test does the same for SSE and gets `429 listener_limit_reached`.
- A test shows that a second address is not affected.
- A test shows that `X-Real-IP` from a peer that is not trusted has no
  effect.
- A test shows the connect rate limit.
- A test shows that a session with no namespace closes after 10 seconds,
  with an injected time source.
- `README.md` documents each setting and the `429`.

## Alternatives Considered

### Limits In nginx Only

Rejected by the operator. The tests and the contract of the relay do not
include an nginx limit. A deployment with no proxy would have no limit.

### One Global Limit

Rejected. It cannot keep a popular event available and also stop one
client.

### `X-Forwarded-For`

Rejected. A client can put each value that it wants in it. nginx sets `X-Real-IP` from the
TCP peer, and the relay accepts it only from a trusted proxy.

### A Limit For Each Event

Rejected in this record. The limit for each client stops one client. A limit
for each event would turn away the listeners of a popular event.

## Consequences

- One client cannot use all the listener connections of the relay.
- Listeners behind one shared address share one limit. An operator can
  increase `MAX_LISTENER_CONNECTIONS_PER_CLIENT`.
- A relay that is exposed directly, with no proxy, uses the TCP peer
  address. That is correct with the default `TRUSTED_PROXIES`.
- The sentence about per-IP rate limiting in `README.md` changes in the
  commit that implements this record.

## References

- AGENTS.md §5
- `README.md` §Environment Variables, §Deployment
- `docs/interoperability.md`
- ADR 0004
- `~/build/musicindex-research/reports/2026-10-17-pixel-palace-curiohoster-show.md`
