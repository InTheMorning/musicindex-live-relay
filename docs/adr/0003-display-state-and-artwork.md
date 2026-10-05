# ADR 0003: Display State And Artwork For Reserved Events

## Status

Accepted - 2026-10-04.

Amended 2026-10-05: "the state before it" in the image retention rule means
the most recent earlier state with a relay image. A state with no relay image,
such as `null` or a URL, no longer removes that image. The limit of two
images for each event does not change.

Amended 2026-10-04: a client that connects to `/display/events` with no
`Last-Event-ID` first gets the present display state. The amendment adds to
the stream and reverses no decision. A client then needs no separate read
before it subscribes.

Accepted 2026-10-04 by the operator. The implementation needs ADR 0001,
because only a reserved event can use these routes.

Proposed 2026-10-04.

`musicindex-live-publisher` ADR 0008 is the broadcaster side. It decides what
the producer and the publisher send. This ADR decides the routes, the limits
and the storage.

## Context

A live event carries one payload, the `remoteValue` block. Podcast apps read
its `image` field, which is a URL. That path does not change.

The operator wants a second, optional path for a private app that plays a
private stream. The stream plays V4V tracks and other tracks. The app shows:

- the Icecast title, which comes in band with the audio,
- the live value, only while a V4V track plays,
- the artwork and the track text for each track, from this new path.

Two facts make the new path different from the live value:

- It exists for tracks that pay nobody. So it cannot ride the payment
  payload.
- It carries image data. The metadata route accepts 64 KiB at most, and the
  other routes have no body limit today.

The relay keeps every event in memory and allows 10,000 active events. An image
store open to every event can use 10,000 times the image limit.

## Decision

### Only A Reserved Event Has A Display Path

A display route answers `409` with `event_not_reserved` for an ephemeral event.
Only the operator makes a reserved event (ADR 0001). So the configured limit of
reserved events bounds the display memory.

### The Display State

The display state of an event is one JSON object:

```json
{
  "track": {
    "artist": "Artist",
    "title": "Title",
    "artwork": { "sha256": "<64 hex characters>", "mime": "image/jpeg" }
  }
}
```

- `track` is `null` when nothing plays. A broadcaster sends `null`
  explicitly.
- `artwork` has one of three forms:
  - `{"sha256": "…", "mime": "…"}` for an image in the store of the relay.
    `mime` is `image/jpeg` or `image/png`.
  - `{"url": "…"}` for an image on a different host. The URL uses `http` or
    `https`, and it has at most 2,048 characters. The relay does not fetch,
    check or store that image. A client loads it.
  - `null` when the track has no image.

### Routes

`PUT /v1/liveitems/{event_id}/artwork/{sha256}`

- It needs the broadcaster token, the same as a publish.
- The body is the image bytes.
- The relay makes three checks. The SHA-256 of the body is `{sha256}`. The
  first bytes are JPEG or PNG. The body is not larger than
  `ARTWORK_MAX_BYTES`, which is 524,288 bytes by default.
- An image that the relay already holds gives `200` with no change.

`POST /v1/liveitems/{event_id}/display`

- It needs the broadcaster token.
- The body is the display state. Its limit is 8 KiB.
- An `artwork.sha256` that the relay does not hold gives `409` with
  `artwork_missing`. So the broadcaster uploads the image first.
- An `artwork.url` with a different scheme, or longer than 2,048 characters,
  gives `400` with `invalid_display`.

`GET /v1/liveitems/{event_id}/display`

- It gives the present display state. It gives `{"track": null}` when the
  event has none.

`GET /v1/liveitems/{event_id}/display/events`

- An SSE stream of `display` events. Each event holds a display state. A
  client can reconnect with `Last-Event-ID`, the same as `/events`.
- A client with no `Last-Event-ID` first gets the present display state, with
  its present `seq` as the `id`. Before the first publish, that state is
  `{"track": null}` with the `id` 0. Added 2026-10-04.
- The stream carries no image bytes.

`GET /v1/liveitems/{event_id}/artwork/{sha256}`

- It gives the image, with its `Content-Type`,
  `Cache-Control: public, max-age=31536000, immutable` and
  `X-Content-Type-Options: nosniff`.
- It gives `404` for an image that the relay does not hold.

The read routes need only the event identifier, the same as `remoteValue`.

Status codes for the write routes:

| Status | Body `error` | Meaning |
|---|---|---|
| `200` | | Done |
| `400` | `sha256_mismatch`, `unsupported_image` or `invalid_display` | The body is not correct |
| `401` | the codes of a publish | No valid bearer token |
| `403` | `invalid_token` | The token does not match |
| `404` | `event_not_found` | The event does not exist |
| `409` | `event_not_reserved` or `artwork_missing` | See above |
| `413` | `payload_too_large` | The body is larger than its limit |
| `429` | `publish_rate_limited` | Too many requests |

### Storage And Lifetime

- The display state and the images stay in memory. Nothing goes to disk. A
  restart gives `{"track": null}` and no image until the next publish.
- For each event, the relay keeps the image of the present display state and
  the image of the most recent earlier state with a relay image. It removes
  every other image.
- A display publish does not renew the lease. Only a publish of the payload
  and a keepalive renew it (ADR 0002).
- When the lease of an event ends, the relay sets its display state to
  `{"track": null}`, sends that state on `/display/events`, and removes its
  images.
- An upload or a display publish uses the rate limit of a publish.

### No Change To The Live Value

`remoteValue`, `/events`, Socket.IO and the metadata route do not change.
Display events never go on `/events` or on Socket.IO. A podcast app sees no
difference.

## Invariants

- Only a reserved event has a display state or images.
- No display state or image reaches disk.
- The display memory is at most the reserved event limit multiplied by two
  images of `ARTWORK_MAX_BYTES`.
- Every write route has a body limit.
- An image is served only with the hash that its bytes have.
- The relay never fetches an image from a URL.
- No display data goes into the live value transports.

## Verification After Implementation

Mechanical, each with a test:

- An ephemeral event gives `409 event_not_reserved` on each display route.
- An upload with a wrong hash, with bytes that are not JPEG or PNG, and over
  the limit each give the correct error.
- A display publish with an unknown image gives `409 artwork_missing`.
- A display state with an `http` or `https` URL is accepted with no upload. A
  `data:` or `file:` URL gives `400 invalid_display`.
- A third image removes the first one.
- A lease expiry sets the state to `{"track": null}`, sends it on SSE, and
  removes the images.
- A display publish does not renew the lease.
- No display event appears on `/events` or on Socket.IO.

## Alternatives Considered

### Display Data In The Live Value

Rejected. The live value exists only for a payable track, and podcast apps
read it. A track that pays nobody would need a payload, and the image would
grow the payload past its limit.

### A Display Path For Every Event

Rejected for now. Its memory has no useful bound while anyone can make an
event. The identity decision in
`docs/research/broadcaster-identity-options.md` can open it later, with a
quota for each credential.

### Images On Disk

Rejected. A display state is current only while the broadcaster runs, the same
as a snapshot. Disk storage adds cleanup work and keeps images after the show.

### A Read Token For The Display Routes

Deferred. An unlisted event identifier has 128 random bits. It is enough while
the stream itself is private. A later ADR can add a read token.

## Consequences

Positive:

- A private app gets artwork and track text for every track, with no change
  for podcast apps.
- An image is uploaded once, and clients can cache it for ever.
- The memory has a fixed bound.

Negative and risks:

- The relay stores image data for the first time. It checks the type, and it
  serves each image with `nosniff`.
- Two more SSE subscribers for each private listener.
- The feature waits for ADR 0001.

## Changes At Acceptance

- `README.md` and `docs/interoperability.md`: add the display routes, in the
  same commit as the code (AGENTS.md).

## References

- `docs/adr/0001-reserved-live-items.md`
- `docs/adr/0002-live-lease.md`
- `docs/research/broadcaster-identity-options.md`
- `musicindex-live-publisher`: `docs/adr/0008-display-path.md`
