# ADR 0008: Read Timeouts For Each Connection

## Status

Proposed 2026-10-21.

Class: durable. AGENTS.md §5 requires a configurable limit with a default for
each resource. This record adds the limits for the time that a client can
hold a connection before it sends a full request.

## Context

A security test on 2026-10-21 ran the relay locally with the defaults. Three
probes held a connection open with no credential:

- A client sent `GET /health HTTP/1.1` and no end of the headers. The
  connection was open after 40 seconds.
- A client sent one request on a keep-alive connection, read the response,
  and sent nothing more. The connection was open after 35 seconds.
- A client sent the headers of `POST /v1/liveitems/{event_id}/metadata`,
  `PUT /v1/liveitems/{event_id}/artwork/{sha256}` and
  `POST /v1/liveitems/reserved` with a `Content-Length`, with only part of
  the body, and with no token. Each connection was open after 35 seconds.
  These handlers read the body before they examine the token.

The cause is in `src/main.rs`. `axum::serve` in axum 0.8.9 gives hyper no
timer. hyper 1.9.0 applies its header read timeout only with a timer, so the
relay has no header timeout. The relay also has no deadline for a request
body.

Each open connection holds a socket and a task. A client that opens many slow
connections can use all the file descriptors of the process. The listener
limits of ADR 0007 do not apply, because these connections are not listener
connections.

On `api.musicindex.org`, nginx is in front of the relay. nginx has its own
client header and body timeouts, and by default it reads the full request
body before it sends the request on. But `README.md` §Deployment permits a
relay that is exposed directly, and the test suite of the relay does not
include an nginx setting. ADR 0007 rejected a limit in nginx only for the
same cause.

The `RequestBodyTimeoutLayer` of `tower-http` 0.6.8 starts its timeout again
after each frame of the body. The relay does not stop a client
that sends one byte before each timeout ends. The layer does not supply a
deadline for the full body.

## Decision

### Two Timeouts

| Setting | Default | Limit |
|---|---|---|
| `HEADER_READ_TIMEOUT_SECS` | `10` | The time from the start of a connection, or from the end of the last response on it, to the end of the request headers. |
| `BODY_READ_TIMEOUT_SECS` | `30` | The time from the end of the request headers to the end of the request body. |

- Each setting is an integer count of seconds. A value of `0`, or a value that
  does not parse, is a startup error that names the setting.
- The header timeout also closes an idle keep-alive connection. One setting
  covers the two conditions, because each is a connection with no request.
- The body timeout is one deadline for the full body. The arrival of data
  does not start it again.
- With the default of 30 seconds, an artwork body of the default
  `ARTWORK_MAX_BYTES` (524,288 bytes) needs about 17.5 KB each second. A
  broadcaster on a slower uplink increases the setting.

### What A Client Gets At A Timeout

- **Headers:** the relay closes the connection. It sends no response,
  because it has no request to answer.
- **Body:** the relay answers `408` with the error code
  `request_body_timeout`, and closes the connection.

### What The Timeouts Do Not Stop

- An open SSE stream, a Socket.IO long-polling request that waits for data,
  and an upgraded WebSocket connection. Each of these has a full request.
  The Engine.IO ping of Socket.IO and the limits of ADR 0007 keep their
  roles.
- The client address of ADR 0007. Each handler continues to get the TCP peer
  address in `ConnectInfo<SocketAddr>`.
- The graceful shutdown in `src/main.rs`.

## Invariants

- A connection with no full request head closes when
  `HEADER_READ_TIMEOUT_SECS` ends.
- An idle keep-alive connection closes when `HEADER_READ_TIMEOUT_SECS` ends.
- A request body that is not complete when `BODY_READ_TIMEOUT_SECS` ends gets
  `408 request_body_timeout`. A slow flow of bytes does not extend the
  deadline.
- No timeout of this record closes a listener stream or an upgraded
  connection.
- Each timeout is configurable and has a default.

## Non-Goals

- A limit on the count of connections that are not listener connections.
  The timeouts make each such connection short.
- A token check before the body is read. It would stop a client with no
  token earlier. A client with a token can also send a slow body. Thus
  this check does not replace a deadline.
- A timeout for the response. A slow reader of an SSE stream is a separate
  subject.

## Before Acceptance

- The operator confirms the defaults: 10 and 30 seconds.
- A spike confirms that the hyper 1.9.0 header read timeout also measures the
  idle time between two requests on one keep-alive connection. If it does
  not, this record gets a separate idle setting before acceptance.
- The operator decides if `docs/interoperability.md` gets the new `408` now
  or in the commit that implements this record. The publisher sends request
  bodies, so it can see the `408`.

## Verification After Implementation

Mechanical. A test proves each item, with short timeouts from the
configuration and a real TCP listener:

- A connection that sends part of a request head closes after the header
  timeout.
- A keep-alive connection that is idle after one response closes after the
  header timeout.
- A body that sends one byte in each half of the body timeout gets
  `408 request_body_timeout` when the full deadline ends.
- An SSE stream stays open after both timeouts end.
- A Socket.IO WebSocket connection stays open after both timeouts end.
- A handler behind the new server loop gets the TCP peer address in
  `ConnectInfo<SocketAddr>`.
- A value of `0` and a value that does not parse are configuration errors
  that name the setting.
- `README.md` documents each setting and the `408`.

Visual. The operator checks each item after the deployment on
`api.musicindex.org`:

- The publisher publishes metadata and uploads artwork with no `408`.
- A Socket.IO app stays connected for 10 minutes.

## Alternatives Considered

### Timeouts In nginx Only

Rejected. The tests and the contract of the relay do not include an nginx
setting. A relay that is exposed directly then has no timeout. ADR 0007
rejected the same alternative for the listener limits.

### The Body Timeout Of `tower-http`

Rejected. It starts again after each frame. The relay then does not stop a
slow flow of bytes.

### A Timeout For The Full Request

Rejected. A `tower-http` `TimeoutLayer` measures the time to a response. It
closes a Socket.IO long-polling request that waits for data. It also does
not stop a slow body on a route that answers before it reads the body.

### A Separate Idle Setting

Rejected unless the spike above shows that hyper needs it. A client sees no
difference between a slow request head and an idle connection. One setting
is easier to operate.

## Consequences

- One client with no credential cannot hold a connection open for longer
  than the two timeouts.
- `src/main.rs` replaces `axum::serve` with a server loop from `hyper-util`
  that gives hyper a timer. The loop keeps the graceful shutdown and
  `ConnectInfo<SocketAddr>`.
- A broadcaster on a very slow uplink can get `408` on an artwork upload. It
  increases `BODY_READ_TIMEOUT_SECS`.
- A client that keeps a connection idle for more than 10 seconds must open a
  new one. HTTP clients do this automatically.

## References

- AGENTS.md §5
- ADR 0007
- `README.md` §Environment Variables, §Deployment
- `docs/interoperability.md`
