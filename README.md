# MusicIndex Live Relay

`musicindex-live-relay` is a small Rust service for relaying live metadata updates over HTTP, Socket.IO, and server-sent events.

The service is intentionally memory-only for v1. Clients create a live item, receive a one-time broadcaster token, publish metadata with that token, and public listeners can receive `remoteValue` updates over Socket.IO. HTTP snapshot reads and SSE are kept as fallback/debug transports.

## Clients And Neighbors

| Component | Repository | Relation |
|---|---|---|
| `musicindex-live-publisher` | `musicindex-live-publisher` | The broadcaster. Sends the direct live value payload. |
| `v4vmm` | `v4vmm` | Registers live items, keeps the event registry and the tokens, and reads snapshots to show what listeners receive. Sends no payloads. |
| Listener apps | external | Read `remoteValue` over Socket.IO, or the HTTP and SSE fallbacks. |

Read `docs/interoperability.md` before you change a route, a body form, or the
state model. Those choices constrain three other repositories.

## Service Routes

By default the relay binds to `127.0.0.1:8018`. It serves these routes at whatever origin and path layout the host chooses to expose:

```text
POST /v1/liveitems
POST /v1/liveitems/reserved
GET  /v1/liveitems/reserved
DELETE /v1/liveitems/reserved/{event_id}
GET  /v1/liveitems/{event_id}/metadata
POST /v1/liveitems/{event_id}/keepalive
GET  /v1/liveitems/{event_id}/remoteValue
GET  /v1/liveitems/{event_id}/events
POST /v1/liveitems/{event_id}/display
GET  /v1/liveitems/{event_id}/display
GET  /v1/liveitems/{event_id}/display/events
PUT  /v1/liveitems/{event_id}/artwork/{sha256}
GET  /v1/liveitems/{event_id}/artwork/{sha256}
GET  /socket.io/*
```

The public URLs advertised in RSS should be based on the deployment's external origin. The `api.musicindex.org` examples below show the MusicIndex deployment, not a hosting requirement.

All live state is process-local. Restarting the process drops ephemeral live items, broadcaster tokens, latest snapshots, and replay buffers.

A reserved live item also writes its identity to a SQLite state file. A restart restores the identity, but not the snapshot. See "Reserve Live Item" below.

## The Lease

Each event holds a lease in memory. A publish or a keepalive renews the
lease and sets `renewed_at` to the current time. The event stays on air
while it holds a snapshot and the time since `renewed_at` is less than
`LEASE_SECS`.

`LEASE_SECS` sets the lease duration. The default is 90 seconds. The value
must be 10 seconds or more. The relay computes the keepalive interval as
`LEASE_SECS / 3`, rounded down, and returns it to the broadcaster. The
broadcaster does not select this interval.

When a lease expires, the relay removes the event snapshot, advances
`seq`, and sends `{}` one time to `remoteValue`, the SSE stream, and
Socket.IO. The event itself stays. A reserved event keeps its identifier
and its token. The next publish brings the event back on air.

A background task checks all leases each second. No transport serves the
removed snapshot after an expiry. The replay buffer does not serve it
either.

## The Listener Timeline

A broadcaster can send the delay of a publish with the
`Listener-Delay-Secs` header (ADR 0004). See "Publish Metadata" below.

Each route reads one of two timelines of the same updates:

| Route | Timeline |
|---|---|
| Socket.IO `remoteValue`, and the value it sends on connect | Listener |
| `GET /v1/liveitems/{event_id}/remoteValue` | Listener |
| `GET /v1/liveitems/{event_id}/events` (SSE) | Instant |
| `GET /v1/liveitems/{event_id}/metadata` | Instant |
| `GET /v1/liveitems/{event_id}/display` and `/display/events` | Instant |

The instant timeline gets an update at the time of its publish. The
listener timeline gets the same update at that time plus the delay.

The listener timeline keeps the order of the instant timeline. A publish
with no header and no waiting update changes the listener value at once.
This is the same behavior as before ADR 0004.

A client that pays from SSE pays before the listener hears the track.
Only a client that waits for an in-band key may pay from SSE.

## API

### Create Live Item

```http
POST /v1/liveitems
```

Creates a new live metadata event. This endpoint is public. `POST /v1/liveitems/` is also accepted for trailing-slash tolerant clients and proxies.

Response:

```json
{
  "event_id": "<random opaque id>",
  "broadcaster_token": "<random secret token>",
  "metadata_url": "/v1/liveitems/<event_id>/metadata",
  "remote_value_url": "/v1/liveitems/<event_id>/remoteValue",
  "events_url": "/v1/liveitems/<event_id>/events",
  "socket_io_url": "/event?event_id=<event_id>"
}
```

The broadcaster token is returned only once. The service stores only a SHA-256 hash of the token and validates it with constant-time comparison.

### Reserve Live Item

```http
POST /v1/liveitems/reserved
Authorization: Bearer <admin_token>
Content-Type: application/json
```

Reserves a live item with a durable identity (ADR 0001). Only the operator
can reserve an item. The request needs the admin token that `ADMIN_TOKEN`
sets. The relay compares the admin token in constant time.

Request body, optional:

```json
{
  "label": "weekly show"
}
```

`label` is an operator name. It is optional. It must not be empty, and it can
hold at most 200 characters. Two reserved items cannot have the same label.
An empty body reserves an item with no label. The body limit is 4 KiB.

Response, with status `201 Created`:

```json
{
  "event_id": "<random opaque id>",
  "broadcaster_token": "<random secret token>",
  "metadata_url": "/v1/liveitems/<event_id>/metadata",
  "remote_value_url": "/v1/liveitems/<event_id>/remoteValue",
  "events_url": "/v1/liveitems/<event_id>/events",
  "socket_io_url": "/event?event_id=<event_id>",
  "label": "weekly show"
}
```

The response has the same fields as `POST /v1/liveitems`, and `label`. When
the request has no label, the response has no `label` field.

The relay returns the broadcaster token one time only. The state file holds
the event identifier, the SHA-256 hash of the token, the label, the creation
time, and the item class. It never holds the token, a payload, a snapshot, a
sequence number, or a replay buffer.

A reserved item uses the same publish, keepalive, read, SSE, and Socket.IO
routes as an ephemeral item. The lease applies to it in the same way.

Status codes:

- `201` when the relay reserved the item. The body holds the token.
- `400` with error code `invalid_body` or `invalid_label` when the body is not valid.
- `401` when the admin token is missing or malformed.
- `403` with error code `invalid_admin_token` when the admin token is wrong.
- `404` with error code `reserved_items_disabled` when no admin token is configured. The feature is off.
- `409` with error code `label_already_reserved` when a reserved item has the label.
- `413` when the request body is larger than 4 KiB.
- `500` with error code `store_unavailable` when the state file write failed. The body holds no other detail.
- `503` with error code `max_reserved_items_reached` when the state file holds `MAX_RESERVED_ITEMS` items.

#### Restart And Idle TTL

At startup, the relay reads each reserved item from the state file. The
stored broadcaster token stays valid after a restart. The relay logs the
state file path and the count of restored items. It logs no identifier and no
hash.

A restored item has no snapshot, because the state file holds no payload. It
is not on air until the next publish:

- `GET /v1/liveitems/{event_id}/remoteValue` gives `200` with `{}`.
- `GET /v1/liveitems/{event_id}/metadata` gives `404` with error code
  `metadata_not_found`, as after a lease expiry.
- A keepalive gives `409` with error code `lease_expired`.
- Socket.IO sends `{}` at connect.

The sequence number of a restored item starts again at zero. The first
publish after a restart gives `seq` 1. The replay buffer is empty. An SSE
client that keeps a `Last-Event-ID` from before the restart gets no replay,
and the next `id` can be lower than its `Last-Event-ID`.

The idle TTL does not remove a reserved item. The lease applies to it. A
lease expiry removes the snapshot, and the item and its identifier stay.

If the state file is corrupt or the relay cannot read it, the relay stops at
startup with a non-zero exit code. The log line `relay stopped with an error`
names the path and the cause. The relay does not delete the file or make it
again.

### List Reserved Items

```http
GET /v1/liveitems/reserved
Authorization: Bearer <admin_token>
```

Returns the reserved items, oldest first (ADR 0001). The request needs the
admin token. The list never holds an ephemeral item. A client that needs a
list of its ephemeral items must keep its own registry.

Response, with status `200 OK`:

```json
{
  "reserved": [
    {
      "event_id": "<random opaque id>",
      "label": "weekly show",
      "created_at": "2026-09-09T18:00:00Z",
      "last_publish_at": "2026-09-09T19:30:00Z"
    }
  ]
}
```

The body is an object that holds the `reserved` array. It is not a bare
array. With no reserved item, the body is `{"reserved": []}`.

- `label` is absent when the item has no label.
- `created_at` is the reserve time. A restart keeps it.
- `last_publish_at` is the time of the last accepted publish. A keepalive
  does not change it. It is absent when the item has not published since the
  relay started, because the relay keeps it in memory only.

The list never holds a broadcaster token or a token hash.

Status codes:

- `200` with the list.
- `401` when the admin token is missing or malformed.
- `403` with error code `invalid_admin_token` when the admin token is wrong.
- `404` with error code `reserved_items_disabled` when no admin token is configured. The feature is off.

### Delete Reserved Item

```http
DELETE /v1/liveitems/reserved/{event_id}
Authorization: Bearer <admin_token>
```

Deletes one reserved item (ADR 0001). The request needs the admin token.

**A delete is permanent.** The relay removes the row from
the state file first, then removes the item from memory. A restart does not
restore the item. The broadcaster token of the item becomes invalid.

After a delete:

- Each route of that identifier answers `404` with error code
  `event_not_found`. This includes `metadata`, `remoteValue`, `events`, a
  publish and a keepalive.
- Each SSE stream of the item ends. A reconnect gets `404`.
- Each Socket.IO client of the item gets `{}` and is disconnected. A
  reconnect gets `{}` and is disconnected, as for each unknown event.

The route never deletes an ephemeral item. An ephemeral identifier gets the
same `404` as an identifier that does not exist, and the item stays.

Status codes:

- `204` when the relay deleted the item. The body is empty.
- `401` when the admin token is missing or malformed.
- `403` with error code `invalid_admin_token` when the admin token is wrong.
- `404` with error code `reserved_items_disabled` when no admin token is configured. The feature is off.
- `404` with error code `event_not_found` when no reserved item has the identifier. An ephemeral identifier gets this answer.
- `500` with error code `store_unavailable` when the state file write failed. The item stays. The body holds no other detail.

The two `404` answers differ in the error code. A client can also tell them
apart from the credential: `reserved_items_disabled` comes before the relay
examines the credential.

### Publish Metadata

```http
POST /v1/liveitems/{event_id}/metadata
Authorization: Bearer <broadcaster_token>
Content-Type: application/json
Listener-Delay-Secs: <optional, a whole number of seconds>
```

Request body:

```json
{
  "event_id": "<same event_id>",
  "metadata": {}
}
```

The path `event_id` and body `event_id` must match.

The wrapped form is a debug and inspection shape. Listener apps read the direct form below, so a broadcaster that wants payment splits to reach listeners must send the direct form. See `docs/interoperability.md`.

For compatibility with the widely used Socket.IO live value implementation, the relay also accepts the live value payload directly:

```json
{
  "title": "This is the title for this block.",
  "image": "https://example.com/art.png",
  "line": ["this is line 1", "this is line 2"],
  "link": {
    "text": "This is the text for the link",
    "url": "https://podcastindex.social"
  },
  "description": "this would be an area for something like show notes",
  "value": {
    "model": {
      "type": "lightning",
      "method": "keysend"
    },
    "destinations": []
  },
  "type": "person",
  "feedGuid": "optional-feed-guid",
  "itemGuid": "optional-item-guid"
}
```

A broadcaster can send the optional request header `Listener-Delay-Secs` with a publish (ADR 0004). The value is a whole number of seconds from 0 to `MAX_LISTENER_DELAY_SECS`. With no header, the delay is 0. The relay keeps the delay of the last accepted publish. A restart clears it, the same as the snapshot.

The delay sets the listener timeline. See "The Listener Timeline" above.

On success, the relay increments the live item's sequence number. It stores the latest snapshot and appends the update to the replay buffer. It sends the same raw payload over SSE for fallback clients. It also updates the listener timeline, and sends the Socket.IO `remoteValue` event when the update reaches that timeline. See "The Listener Timeline" above.

The publish also renews the event lease. See "The Lease" above.

Response:

```json
{
  "event_id": "<event_id>",
  "accepted": true,
  "seq": 1,
  "lease_secs": 90,
  "keepalive_interval_secs": 30
}
```

`lease_secs` is the current `LEASE_SECS` value. `keepalive_interval_secs` is the maximum time between a publish and the next keepalive.

Status codes:

- `400` when the path and body event IDs differ.
- `400` with error code `invalid_listener_delay` when the `Listener-Delay-Secs` value is not a whole number from 0 to `MAX_LISTENER_DELAY_SECS`.
- `400` with the same error code when the request sends `Listener-Delay-Secs` more than one time.
- `401` when the bearer token is missing or malformed.
- `403` when the bearer token is wrong.
- `404` when the event does not exist.
- `413` when the request body exceeds 64 KiB.
- `429` when per-event publish rate limit is exceeded. A bad `Listener-Delay-Secs` header does not use a slot of this limit.

The `Bearer` scheme is matched case-insensitively (`bearer`, `BEARER`, etc.).

The relay distinguishes the wrapped form from the direct form by exact key match: a request body is treated as wrapped only when the JSON object has exactly the two keys `event_id` and `metadata`. Any object with additional keys (or different keys) is treated as a direct live value payload.

### Keep The Event Alive

```http
POST /v1/liveitems/{event_id}/keepalive
Authorization: Bearer <broadcaster_token>
```

A broadcaster sends a keepalive between publishes to keep the event lease
alive. The relay ignores the request body. Send the request with no body.

Response:

```json
{
  "event_id": "<event_id>",
  "lease_expires_at": "2026-09-27T18:00:00Z",
  "keepalive_interval_secs": 30
}
```

Status codes:

- `401` when the bearer token is missing or malformed.
- `403` when the bearer token is wrong.
- `404` when the event does not exist.
- `409` with error code `lease_expired` when the event holds no snapshot. Publish to bring the event back on air.
- `429` when the per-event publish rate limit is exceeded. A keepalive shares this limit with a publish.

The `Bearer` scheme is matched case-insensitively (`bearer`, `BEARER`, etc.), the same as a publish.

A keepalive never restores a removed snapshot. Only a publish brings an event back on air.

### Read Latest Metadata

```http
GET /v1/liveitems/{event_id}/metadata
```

Returns the latest published metadata snapshot. This route always follows
the instant timeline (ADR 0004). See "The Listener Timeline" above.

Response:

```json
{
  "event_id": "<event_id>",
  "seq": 1,
  "updated_at": "2026-04-27T12:34:56Z",
  "renewed_at": "2026-04-27T12:34:56Z",
  "lease_expires_at": "2026-04-27T12:36:26Z",
  "metadata": {}
}
```

`renewed_at` is the time of the last publish or keepalive. `lease_expires_at` is `renewed_at` plus `LEASE_SECS`.

Returns `404` when the event does not exist, no metadata has been
published yet, or the event lease has expired. See "The Lease" above. A
reserved item that a restart restored has no metadata until its next publish.

The Socket.IO-compatible raw payload is also available at:

```http
GET /v1/liveitems/{event_id}/remoteValue
```

When the event exists but no metadata has been published yet, this endpoint returns `200 OK` with `{}` (mirrors Socket.IO's initial-emit behavior). It returns `404` only when the event itself does not exist. This route always follows the listener timeline (ADR 0004). See "The Listener Timeline" above.

### Subscribe With Socket.IO

Socket.IO is the primary app-facing transport because it matches the existing live value implementation used by The Split Kit.

Connect to namespace `/event` with `event_id` in the query string. Replace the origin with the relay host's public origin:

```js
import { io } from "socket.io-client";

const socket = io("https://api.musicindex.org/event", {
  query: { event_id: "<event_id>" }
});

socket.on("remoteValue", payload => {
  applyRemoteValue(payload);
});
```

The relay immediately emits the current `remoteValue` payload after a successful connection. If no metadata has been published yet, it emits `{}`. This is the listener timeline value, not always the newest published payload. See "The Listener Timeline" above. A publish with a delay reaches this emit after the delay. Future publishes emit the raw live value payload:

```json
{
  "title": "...",
  "image": "...",
  "line": ["..."],
  "value": {
    "model": {
      "type": "lightning",
      "method": "keysend"
    },
    "destinations": []
  },
  "type": "person"
}
```

The Socket.IO server uses namespace `/event` and the standard Engine.IO request path `/socket.io`.

### Subscribe With SSE

```http
GET /v1/liveitems/{event_id}/events
Accept: text/event-stream
```

The SSE stream is public and is intended as a fallback/debug transport. Each metadata update is emitted with the same event name and data shape used by the Socket.IO live value implementation:

```text
event: remoteValue
id: <seq>
data: {"title":"...","image":"...","line":["..."],"value":{...},"type":"person"}
```

When `Last-Event-ID` is present and valid, the relay replays buffered events with `seq > Last-Event-ID` before streaming live updates. The replay buffer keeps the last 100 metadata updates per live item.

The replay buffer is in memory only. After a restart, a reserved item has an empty replay buffer, and its `seq` starts again at zero. See "Restart And Idle TTL" above.

The stream also sends periodic keepalive comments.

SSE always follows the instant timeline (ADR 0004). It never waits for
the delay of a publish. See "The Listener Timeline" above.

### Display State

ADR 0003 adds an optional display path for a private app. Only a reserved
item has a display state. The display path does not share data with the live
value. `remoteValue`, `/events`, Socket.IO and the metadata route do not send
a display state. A display route does not send a live value.

A display state is one JSON object:

```json
{
  "track": {
    "artist": "Artist",
    "title": "Title",
    "artwork": { "sha256": "<64 lowercase hex characters>", "mime": "image/jpeg" },
    "songLine": "Artist - Title",
    "value": { "eventGuid": "event123", "blockGuid": "block456" },
    "album": "Album Name"
  }
}
```

- The object has only the key `track`.
- `track` is `null` when nothing plays. Send `null` explicitly.
- A `track` object has the required keys `artist`, `title` and `artwork`.
  It can optionally have `songLine`, `value` and `album`. `artist` and `title`
  are strings.
- `songLine` is an optional string of 1 to 1,024 characters. It is the ICY
  title line that the broadcaster sends (ADR 0005).
- `value` is an optional object with only the keys `eventGuid` and
  `blockGuid`. Each is a string of 1 to 128 characters. It names the live
  value block of the track (ADR 0005).
- `album` is an optional string of 1 to 1,024 characters. It is the album of
  the track (ADR 0006).
- `artwork` has one of three forms:
  - `{"sha256": "…", "mime": "…"}`. `sha256` is 64 lowercase hexadecimal
    characters. `mime` is `image/jpeg` or `image/png`. The relay must hold
    the image for the item, and `mime` must be its stored type. Upload the
    image first. See "Upload An Image" below.
  - `{"url": "…"}`. The URL uses `http` or `https`, and it has at most 2,048
    characters. The relay does not fetch the image. A client loads it.
  - `null` when the track has no image.

The display state and the images are in memory only. A restart gives
`{"track": null}`, a display `seq` of zero, an empty display replay buffer and
no image.

When the lease of an item expires, the relay also sets its display state to
`{"track": null}` and sends that state on `/display/events`. If the display
state is `{"track": null}` at the expiry, the relay sends nothing more. The
relay also removes every image of the item, with or without a display state.
A display publish and an image upload do not renew the lease. See "The Lease"
above.

The relay examines the leases one time each second. An item that has no live
lease keeps no image. The relay removes an upload to such an item in one
second or less. To keep images, publish the live value and send keepalives.

#### Publish The Display State

```http
POST /v1/liveitems/{event_id}/display
Authorization: Bearer <broadcaster_token>
Content-Type: application/json
```

The body is a display state. The body limit is 8 KiB. A display publish uses
the per-event publish rate limit, the same limit as a publish and a
keepalive.

Response:

```json
{
  "event_id": "<event_id>",
  "accepted": true,
  "seq": 1
}
```

`seq` is the sequence number of the display stream. It is not the `seq` of
the live value.

Status codes:

- `200` when the relay accepts the display state.
- `400` with error code `invalid_display` when the body is not a display
  state. This includes a URL with a scheme that is not `http` or `https`,
  a URL that is longer than 2,048 characters, and a `mime` that is not the
  stored type of the image.
- `401` with error code `missing_bearer_token`, `invalid_bearer_token` or
  `invalid_authorization_header` when the bearer token is missing or
  malformed. These are the codes of a publish.
- `403` with error code `invalid_token` when the bearer token is wrong.
- `404` with error code `event_not_found` when the item does not exist.
- `409` with error code `event_not_reserved` when the item is ephemeral.
- `409` with error code `artwork_missing` when the relay does not hold the
  image of `artwork.sha256` for the item. Upload the image, then publish
  again.
- `413` when the body is larger than 8 KiB. The error code is
  `payload_too_large`. A request with a `Content-Length` header over the
  limit gets `413` with a plain text body.
- `429` with error code `publish_rate_limited` when the per-event publish
  rate limit is exceeded.

The relay examines the credential before the body. A body error does not come
before a `401`, `403`, `404` or `409 event_not_reserved` answer. The relay
examines `artwork_missing` and the stored type last, after the rate limit.

#### Read The Display State

```http
GET /v1/liveitems/{event_id}/display
```

Returns the present display state, with its display sequence number:

```json
{
  "seq": 12,
  "track": {"artist": "Artist", "title": "Title", "artwork": null}
}
```

- `seq` is the `id` of the same state on `/display/events`. A client can read
  this route, and then open the stream with `Last-Event-ID` set to a lower
  value to get the states before it. The replay buffer holds the last 100
  states.
- Before the first publish, and after a restart, the body is
  `{"seq": 0, "track": null}`.
- `track` has the shape of §The Display State. Ignore an unknown top-level
  key.

Status codes:

- `200` with the display state.
- `404` with error code `event_not_found` when the item does not exist.
- `409` with error code `event_not_reserved` when the item is ephemeral.

#### Subscribe To The Display State

```http
GET /v1/liveitems/{event_id}/display/events
Accept: text/event-stream
```

Each display state comes as one SSE event:

```text
event: display
id: <display seq>
data: {"track":{"artist":"Artist","title":"Title","artwork":null}}
```

The stream has its own sequence number and its own replay buffer of the last
100 display states. When `Last-Event-ID` is present and valid, the relay
replays the display states with a display `seq` greater than that value. With
no `Last-Event-ID`, the relay first sends the present display state, with its
present display `seq` as the `id`. Before the first publish, that state is
`{"track": null}` with the `id` 0. A client thus needs no separate read before
it subscribes. A state is never sent twice on one stream. The stream sends no
image bytes. A delete of the item ends the stream.

Status codes:

- `200` with the stream.
- `404` with error code `event_not_found` when the item does not exist.
- `409` with error code `event_not_reserved` when the item is ephemeral.
- `503` with error code `max_sse_connections_reached` when the relay has
  `MAX_SSE_CONNECTIONS` streams open. The display streams and the `/events`
  streams share this limit.

#### Upload An Image

```http
PUT /v1/liveitems/{event_id}/artwork/{sha256}
Authorization: Bearer <broadcaster_token>
```

The body is the image bytes. `{sha256}` is the SHA-256 of the body, as 64
lowercase hexadecimal characters. The body must start with the JPEG bytes
`FF D8 FF` or the PNG bytes `89 50 4E 47 0D 0A 1A 0A`. The relay gets the
image type from these bytes only. It ignores the `Content-Type` header of
the request.

The body limit is `ARTWORK_MAX_BYTES`, 524,288 bytes by default. An upload
uses the per-event publish rate limit, the same limit as a publish.

Response:

```json
{
  "event_id": "<event_id>",
  "sha256": "<64 lowercase hex characters>",
  "mime": "image/jpeg",
  "stored": true
}
```

`mime` is the type that the relay read from the first bytes. `stored` is
`false` when the item held the image already. The relay then changes nothing.

Status codes:

- `200` when the relay holds the image after the request.
- `400` with error code `sha256_mismatch` when `{sha256}` is not the SHA-256
  of the body. This includes a `{sha256}` that is not 64 lowercase
  hexadecimal characters.
- `400` with error code `unsupported_image` when the body does not start
  with the JPEG bytes or the PNG bytes.
- `401` with error code `missing_bearer_token`, `invalid_bearer_token` or
  `invalid_authorization_header` when the bearer token is missing or
  malformed.
- `403` with error code `invalid_token` when the bearer token is wrong.
- `404` with error code `event_not_found` when the item does not exist.
- `409` with error code `event_not_reserved` when the item is ephemeral.
- `413` when the body is larger than `ARTWORK_MAX_BYTES`. The error code is
  `payload_too_large`. A request with a `Content-Length` header over the
  limit gets `413` with a plain text body.
- `429` with error code `publish_rate_limited` when the per-event publish
  rate limit is exceeded.

The relay examines the credential before the body, as for a display publish.

#### Read An Image

```http
GET /v1/liveitems/{event_id}/artwork/{sha256}
```

Returns the image bytes with these headers:

```text
Content-Type: image/jpeg
Cache-Control: public, max-age=31536000, immutable
X-Content-Type-Options: nosniff
```

`Content-Type` is the stored type, `image/jpeg` or `image/png`. The bytes of
a path never change, so a client can keep an image in its cache.

Status codes:

- `200` with the image.
- `404` with error code `artwork_not_found` when the relay does not hold the
  image for the item.
- `404` with error code `event_not_found` when the item does not exist.
- `409` with error code `event_not_reserved` when the item is ephemeral.

#### Image Retention

An item holds two images at most. So the images that the relay holds use at
most `MAX_RESERVED_ITEMS` multiplied by two images of `ARTWORK_MAX_BYTES`.

- After a display publish, the item keeps the image of the present display
  state and the image of the most recent earlier state with a relay image. A
  state with no relay image, such as `null` or a URL, does not remove that
  image. The relay removes every other image.
- An upload never makes the item hold more than two images. An upload can
  add a new image to an item that holds two images. The relay then first
  removes one image that the present display state does not name. It removes the
  image of the state before the present state, if the item holds it. Else it
  removes the earlier upload.
- An upload never removes the image of the present display state.
- A lease expiry removes every image of the item.

An image can go before a display state names it. A broadcaster that gets
`409 artwork_missing` uploads the image again and publishes again.

## RSS `podcast:liveValue`

The relay is intended to be advertised from a Podcasting 2.0 live item with an
experimental `podcast:liveValue` tag. This follows the same idea as The Split
Kit's live value work: the RSS feed tells podcast apps which live metadata
transport to use and where to connect, then the live server sends the current
chapter/value block while the stream is playing.

For this relay, the recommended RSS shape is:

```xml
<podcast:liveItem
    status="live"
    start="2026-04-28T00:00:00Z"
    end="2026-04-28T03:00:00Z">

  <title>My Live Music Show</title>
  <guid isPermaLink="false">my-show-live-2026-04-28</guid>

  <enclosure
      url="https://stream.example.com/live.mp3"
      type="audio/mpeg"
      length="1" />

  <podcast:contentLink href="https://example.com/live">
    Listen live
  </podcast:contentLink>

  <podcast:value type="lightning" method="keysend" suggested="0.00000015000">
    <podcast:valueRecipient
        name="Default show split"
        type="node"
        address="030000000000000000000000000000000000000000000000000000000000000000"
        split="100" />
  </podcast:value>

  <podcast:liveValue
      uri="https://api.musicindex.org/event?event_id=GCeSvW8qLdzSdXO9rrlKGg"
      protocol="socket.io" />
</podcast:liveItem>
```

Only the public event URI belongs in RSS. The broadcaster token returned by
`POST /v1/liveitems` is a write credential and must remain private. The relay
uses Socket.IO as the compatibility transport and emits the current
`remoteValue` payload on the `/event` namespace.

### Event URI Convention

Use the Socket.IO namespace URI from the create response in the tag:

```xml
<podcast:liveValue
    uri="https://api.musicindex.org/event?event_id=<event_id>"
    protocol="socket.io" />
```

Apps connect with the standard Socket.IO client:

```js
const socket = io("https://api.musicindex.org/event", {
  query: { event_id: "<event_id>" }
});

socket.on("remoteValue", applyRemoteValue);
```

The HTTP fallback/debug endpoints are:

```text
GET /v1/liveitems/<event_id>/metadata     wrapped current snapshot
GET /v1/liveitems/<event_id>/remoteValue  raw current remoteValue payload
GET /v1/liveitems/<event_id>/events       SSE stream of remoteValue events
```

The relay sends the current payload immediately on Socket.IO connect. The HTTP
snapshot endpoints are still useful for debugging, startup checks, and mobile
background modes where the app may choose to poll instead of keeping a live
connection open. If Socket.IO disconnects, the Socket.IO client handles
reconnection. Apps can also poll `/remoteValue` or `/metadata` as a conservative
fallback.

The direct SSE URL form is also possible:

```xml
<podcast:liveValue
    uri="https://api.musicindex.org/v1/liveitems/<event_id>/events"
    protocol="sse" />
```

The Socket.IO form is preferred for app compatibility. The SSE form remains
available for clients that deliberately choose the simpler one-way transport.

### `remoteValue` Payload

Published metadata should use the same shape as the Socket.IO `remoteValue`
payload. It acts as a live chapter plus a replacement value block, similar to a
`podcast:valueTimeSplit` in a recorded show, except the active block changes in
real time instead of being known ahead of time.

Recommended payload shape:

```json
{
  "title": "This is the title for this block.",
  "image": "https://example.com/art.png",
  "line": [
    "this is line 1",
    "this is line 2",
    "this is line 3",
    "this is line 4"
  ],
  "link": {
    "text": "This is the text for the link",
    "url": "https://podcastindex.social"
  },
  "description": "this would be an area for something like show notes",
  "value": {
    "model": {
      "type": "lightning",
      "method": "keysend"
    },
    "destinations": [
      {
        "name": "The Split Kit",
        "address": "030a58b8653d32b99200a2334cfe913e51dc7d155aa0116c176657a4f1722677a3",
        "customKey": "696969",
        "customValue": "boPNspwDdt7axih5DfKs",
        "split": "5",
        "fee": "false"
      }
    ]
  },
  "type": "person",
  "feedGuid": "optional-feed-guid",
  "itemGuid": "optional-item-guid"
}
```

When this payload is active, supporting apps should use `value` as the
current payment split. The static `<podcast:value>` inside the live item remains
the fallback/default split for apps that do not support `podcast:liveValue`, for
periods before the first live update, and for reconnect failures.

### Health

```http
GET /health
```

Returns:

```text
ok
```

`GET /v1/liveitems/health` returns the same response and is useful for checking that nginx is reaching the live relay specifically. On `api.musicindex.org`, the top-level `/health` route may belong to another API service.

## Configuration

Configuration is read from environment variables.

| Variable | Default | Description |
| --- | --- | --- |
| `BIND` | `127.0.0.1:8018` | Socket address to listen on. |
| `MAX_ACTIVE_EVENTS` | `10000` | Maximum number of live items kept in memory. |
| `MAX_SSE_CONNECTIONS` | `1000` | Maximum concurrent SSE streams. |
| `EVENT_TTL_SECS` | `86400` | Time before inactive events expire. A reserved item does not expire. |
| `MAX_CREATES_PER_SEC` | `50` | Global cap on `POST /v1/liveitems` per second. Returns 429 when exceeded. |
| `MAX_PUBLISHES_PER_EVENT_PER_SEC` | `20` | Per-event cap on metadata publishes per second. Keepalives, display publishes and image uploads use the same cap. Returns 429 when exceeded. |
| `LEASE_SECS` | `90` | Lease duration for an event. Must be 10 or more. See "The Lease" above. |
| `ADMIN_TOKEN` | not set | The operator credential for the reserve, list and delete routes of reserved items. It must have 16 bytes or more. When it is not set, these routes answer `404` and the relay opens no state file. |
| `STATE_FILE` | `/var/lib/musicindex-live-relay/reserved-items.sqlite3` | The SQLite file for reserved items. The relay opens it and restores the reserved items only when `ADMIN_TOKEN` is set. The parent directory must exist and be writable. |
| `MAX_RESERVED_ITEMS` | `100` | Maximum number of reserved items in the state file. |
| `ARTWORK_MAX_BYTES` | `524288` | The body limit of an image upload, in bytes. A reserved item holds two images at most. See "Image Retention" above. |
| `MAX_LISTENER_DELAY_SECS` | `300` | The highest value of the `Listener-Delay-Secs` header that a publish can send (ADR 0004). See "Publish Metadata" above. |
| `MAX_PENDING_LISTENER_UPDATES` | `64` | The highest count of pending listener updates that the relay keeps for each event (ADR 0004). Must be 1 or more. See "The Listener Timeline" above. |

The included systemd unit sets `ProtectSystem=strict`,
`StateDirectory=musicindex-live-relay` and `StateDirectoryMode=0700`. systemd
makes `/var/lib/musicindex-live-relay/` with the correct owner and mode 0700,
and the relay can write the state file there. If the relay cannot open or read
the state file, it stops at startup and the error names the path.

Metadata request bodies are limited to 64 KiB. Reserve request bodies are limited to 4 KiB. Display request bodies are limited to 8 KiB. Image uploads are limited to `ARTWORK_MAX_BYTES`. Other routes are unconstrained.

Per-IP rate limiting is the responsibility of the front-end proxy (e.g. nginx). The relay's `MAX_CREATES_PER_SEC` is a global safety bound, not a per-client limit.

CORS is enabled with a permissive policy (any origin, method, and header) so browser-side podcast apps can consume the SSE and HTTP fallback endpoints directly. Lock this down at the proxy if you need stricter origin policy.

## Run

```sh
cargo run
```

With explicit configuration:

```sh
BIND=127.0.0.1:8018 MAX_ACTIVE_EVENTS=10000 cargo run
```

## Test

```sh
cargo fmt -- --check
cargo check
cargo test
cargo clippy -- -D warnings
```

## Deployment

A systemd unit template is included at:

```text
systemd/musicindex-live-relay.service
```

The unit runs `/usr/local/bin/musicindex-live-relay`, sets `BIND=127.0.0.1:8018`, gives the relay the state directory `/var/lib/musicindex-live-relay/`, and restarts on failure. Five failed starts in 300 seconds stop the restart loop, and systemd marks the unit `failed`. A corrupt state file causes this. `docs/runbooks/reserved-live-items.md` gives the recovery.

The relay can be exposed directly or behind any reverse proxy that preserves the service routes. MusicIndex currently deploys it behind nginx at `api.musicindex.org`; that deployment uses:

- `/v1/liveitems` proxies to `127.0.0.1:8018`.
- `/v1/liveitems/` proxies to `127.0.0.1:8018`.
- `/socket.io/` proxies to `127.0.0.1:8018`.
- Other API routes continue to the existing Stophammer service.

Example nginx locations for that split deployment:

```nginx
location = /v1/liveitems {
    proxy_pass http://127.0.0.1:8018;
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
}

location ^~ /socket.io/ {
    proxy_pass http://127.0.0.1:8018;
    proxy_http_version 1.1;
    proxy_buffering off;
    proxy_cache off;
    proxy_read_timeout 1h;

    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
}

location ^~ /v1/liveitems/ {
    proxy_pass http://127.0.0.1:8018;
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;

    proxy_buffering off;
    proxy_cache off;
    proxy_read_timeout 1h;

    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
}
```

The `proxy_pass` target intentionally has no trailing slash so nginx preserves the relay paths when forwarding to the service.

If nginx returns `502 Bad Gateway` for `/v1/liveitems`, nginx is matching the route but cannot reach the relay on `127.0.0.1:8018`. Check the service on the host:

```sh
sudo systemctl status musicindex-live-relay --no-pager
sudo journalctl -u musicindex-live-relay -n 100 --no-pager
ss -ltnp | grep 8018
curl -i http://127.0.0.1:8018/health
curl -i -X POST http://127.0.0.1:8018/v1/liveitems
curl -i 'http://127.0.0.1:8018/socket.io/?EIO=4&transport=polling'
```

After the relay is listening locally, reload nginx and verify the proxied health route:

```sh
sudo nginx -t
sudo systemctl reload nginx
curl -i https://api.musicindex.org/v1/liveitems/health
curl -i -X POST https://api.musicindex.org/v1/liveitems
```
