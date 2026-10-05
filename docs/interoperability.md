# Interoperability

This relay is one part of a chain that four repositories build separately. This
document says which behavior of this service other repositories depend on, and
which limits they must work around.

The full chain map lives in `v4vmm`:
`docs/architecture/broadcast-chain.md`.

## The Chain

```text
v4vmm ── writes MusicIndex tags into the audio file
             │
             ▼
a player plays the file (Mixxx, mpv, or liquidsoap)
             │
             ▼
a producer writes a musicindex.nowplaying/1 drop file
             │
             ▼
musicindex-live-publisher sends the direct live value payload
             │
             ▼
THIS SERVICE holds the payload and sends remoteValue
             │
             ├──▶ listener apps
             └──▶ v4vmm, to show what listeners receive
```

`musicindex-live-publisher` is the only sender. `v4vmm` creates live items and
reads snapshots, and sends no payloads.

## What Consumers Depend On

### Two body forms, separated by exact key match

A body with exactly the keys `event_id` and `metadata` is the wrapped form. Any
other object is a direct live value payload, and the relay passes it through
without interpretation.

Listener apps read the direct form and find payment splits in
`value.destinations`. A broadcaster that sends the wrapped form reaches no
listener with its splits, and this service reports no error for that.

`musicindex-live-publisher` has a unit test that asserts its payload never has
exactly the keys `event_id` and `metadata`. Do not change the separation rule
without a change in that repository.

### The create response fields

`POST /v1/liveitems` returns `event_id`, `broadcaster_token`, `metadata_url`,
`remote_value_url`, `events_url`, and `socket_io_url`. `v4vmm` stores these
fields in its event registry, because this service cannot list them later.

### A `404` means a dead event

`v4vmm` tests a stored event with `GET /v1/liveitems/{event_id}/metadata`. A
`404` tells the operator that the event no longer exists. Keep that status code
stable.

A lease expiry also gives a `404`, with the error code `metadata_not_found`.
The event continues to exist, and a new publish brings it back on air.
See "A lease expires" below.

## Limits That Consumers Work Around

### State is in memory

A restart of this process discards ephemeral live items, their broadcaster
tokens, every latest snapshot, and every replay buffer. Every ephemeral event
dies, its stored token becomes invalid, and listeners must tune to a new event
identifier. A reserved item survives a restart. See "Two classes of live item"
below.

`v4vmm` therefore keeps its own registry of the events that it created, and
reports a dead event to the operator instead of a silent replacement.

### Two classes of live item

ADR 0001 adds a reserved live item. `POST /v1/liveitems/reserved` makes one,
and it needs the admin token. With no admin token configured, that route
answers `404` and the relay has ephemeral items only.

- An ephemeral item does not change. A restart or the idle TTL removes it.
  `POST /v1/liveitems` never writes to disk.
- A reserved item writes its identity and its token hash to a SQLite state
  file. The file never holds a payload, a snapshot, or a replay buffer.

A reserved item survives a restart and the idle TTL. Its stored broadcaster
token stays valid. The relay restores its identity, but not its snapshot, so
after a restart it serves `{}` until the next publish:

- `remoteValue` and Socket.IO give `{}`.
- `GET /v1/liveitems/{event_id}/metadata` gives `404` with the error code
  `metadata_not_found`, as after a lease expiry. The event exists.
- A keepalive gives `409` with the error code `lease_expired`. The
  broadcaster must publish to go back on air.
- The sequence number starts again at zero, and the replay buffer is empty.
  An SSE client that keeps a `Last-Event-ID` sees the reset.

The lease applies to a reserved item. A lease expiry removes the snapshot,
and the item stays. A corrupt state file stops the relay at startup. The
included systemd unit stops the restart loop after five failed starts, and
the unit becomes `failed`.

The reserve response holds the fields of the create response, and `label`.
`v4vmm` parses `event_id`, `broadcaster_token`, `metadata_url`, and
`events_url` from it. Do not rename them. The status codes `201`, `401`,
`403`, `404`, and `409` each have one meaning. See `README.md`.

### An idle event expires

The reaper removes an ephemeral event when its last activity is older than the
TTL. The default is 24 hours.

An ephemeral event therefore dies with no restart. Consumers see the same
`404` for both death modes, so no consumer needs to separate them. The reaper
never removes a reserved item, so a weekly show or a station uses a reserved
item.

### A lease expires

ADR 0002 adds a lease to each event. A publish or a keepalive renews the
lease. The broadcaster can stop before the idle TTL passes, and the event
then goes off air immediately.

A lease expiry removes the snapshot but keeps the event. `GET
/v1/liveitems/{event_id}/metadata` then gives `404` with the error code
`metadata_not_found`, not `event_not_found`. A client that reads the error
code can tell a lease expiry from a removed event. A client that reads only
the status code cannot.

For `v4vmm`, a lease expiry means the event is off air. It does not mean
the event is dead. The event identifier and the stored token do not become
invalid. A broadcaster that resumes publishing brings the same event back
on air with no new identifier.

### List and delete apply to reserved items only

ADR 0001 adds two admin routes. `GET /v1/liveitems/reserved` lists the
reserved items. `DELETE /v1/liveitems/reserved/{event_id}` deletes one
reserved item permanently. Both routes need the admin token. With no admin
token configured, both answer `404` with the error code
`reserved_items_disabled`.

A control surface can list and delete reserved items:

- The list is an object that holds a `reserved` array. Each item holds
  `event_id`, `created_at`, and `label` and `last_publish_at` when they have a
  value. The list never holds a token or a token hash.
- `last_publish_at` is in memory only. It is absent after a restart until the
  next publish.
- A delete answers `204`. After it, each route of that identifier answers
  `404` with the error code `event_not_found`. SSE streams of the item end,
  and Socket.IO clients get `{}` and are disconnected.

Neither route reports an ephemeral item. An ephemeral identifier gets the same
`404` as an absent identifier, and the delete removes nothing. A client that
needs a list of its ephemeral items must keep its own registry. For an
ephemeral item, `Delete` in a control surface means "forget locally".
This service discards its own record at the next restart or after the idle
TTL.

### The display state and the images are in memory only

ADR 0003 adds three display routes and two artwork routes for a reserved
item. Tasks 001 and 002 of the display state plan implement them:

- `POST /v1/liveitems/{event_id}/display` publishes a display state. It needs
  the broadcaster token.
- `GET /v1/liveitems/{event_id}/display` gives the present display state.
- `GET /v1/liveitems/{event_id}/display/events` is an SSE stream of `display`
  events, with its own `seq` and its own replay buffer. A client with no
  `Last-Event-ID` first gets the present display state. `/events` does not do
  this.
- `PUT /v1/liveitems/{event_id}/artwork/{sha256}` uploads an image. It needs
  the broadcaster token. The body limit is `ARTWORK_MAX_BYTES`, 524,288 bytes
  by default.
- `GET /v1/liveitems/{event_id}/artwork/{sha256}` gives an image, with its
  stored type, a cache time of one year and `nosniff`.

`musicindex-live-publisher` ADR 0008 is the sender. A private app is the
reader. `README.md` gives the bodies, the limits and each status code.

A consumer must know these facts:

- An ephemeral item gets `409` with the error code `event_not_reserved` on
  each display route and each artwork route. An absent item gets
  `404 event_not_found`.
- The display state and the images are in memory only. After a restart, the
  read gives `{"track": null}` and the item holds no image. The display `seq`
  starts again at zero, and the display replay buffer is empty.
- A display publish and an upload do not renew the lease. When the lease
  expires, the relay sets a display state that is not `{"track": null}` to
  `{"track": null}`, and sends it on `/display/events`. It also removes every
  image of the item. An item with no live lease keeps an upload for one
  second at most.
- The display path does not change the live value. `remoteValue`, `/events`,
  Socket.IO and the metadata route do not send a display state. The display
  `seq` and the live value `seq` are different counters.
- An `artwork.sha256` in a display state must name an image that the item
  holds. Else the publish gets `409 artwork_missing`. Its `mime` must be the
  stored type, else the publish gets `400 invalid_display`. The relay gets
  the type from the first bytes of the image only.
- The upload checks the SHA-256 of the body against the path, and gives
  `400 sha256_mismatch` when they are not the same. A body that is not JPEG
  or PNG gets `400 unsupported_image`. A second upload of a held image gives
  `200` with `stored: false` and no change.

Image retention keeps the memory in its limit:

- An item holds two images at most.
- After a display publish, the item keeps the image of the present state and
  the image of the most recent earlier state with a relay image. The relay
  removes every other image. A client that is behind the stream can thus
  still load the image of the last track after a `null` state.
- An upload to an item that holds two images first removes one image that
  the present state does not name. That is the image of the state before the
  present state, else the earlier upload. An upload never removes the image
  of the present state. Added 2026-10-04 by the task 002 review, so that many
  uploads with no publish cannot hold more than two images (ADR 0003
  §Invariants).
- So an image can go before a display state names it. The broadcaster then
  gets `409 artwork_missing`, uploads the image again and publishes again.
  `musicindex-live-publisher` display task 004 does this one time.

### The token is returned one time

This service stores a SHA-256 hash of the broadcaster token and compares it in
constant time. It cannot return the token again.

A client that loses the token loses the event. `v4vmm` writes each token to a
file with mode `0600` for this reason, and never puts a token in a database or
a log.

## Requested Work

### Long-lived live items

A weekly show and a permanent station both need an event that survives a
restart of this process and the idle TTL. An ephemeral event does not.

ADR 0001 in this repository records the decision, and it is Implemented on
2026-10-04. A reserved item survives a restart and the idle TTL, and the
operator can list and delete reserved items. The review is
`docs/reviews/adr-0001-implementation-review.md`. The operator procedures are
in `docs/runbooks/reserved-live-items.md`.

The requirement, from the 2026-09-06 chain review:

- An option to reserve a live item that survives a process restart.
- Durable storage for the identifier and the token hash of a reserved item.
- A clear difference between a short event, for one show, and a reserved
  event, for a station.

This is a decision for this repository. `v4vmm` ADR 0059 records it as
follow-up work and its event registry already stores what a resume needs.

### Per-transport delay for live payloads

`musicindex-live-publisher` holds each payload for a configured stream delay
before it sends the payload here. The delay exists so a live listener's app
flips the value block near the moment that listener hears the track change,
after the encoder, the icecast queue, and the player buffer.

That delay is a property of live delivery. It is not a property of the audio.
Applying it before the payload reaches this service means every consumer
receives the delayed version, including a snapshot read that a control surface
uses to show what listeners receive.

The question for this repository: should the delay be applied here, on the
socket.io emission only, instead of at the publisher? The publisher would then
send on sight, this service would hold the socket.io emission for the target
delay, and the HTTP snapshot would show the current truth without a delay.

Not decided. It needs a measurement of the real post-icecast delay first,
which nobody has taken. Recorded so the option is not lost.

## References

- `README.md` in this repository for the full API
- `v4vmm`: `docs/architecture/broadcast-chain.md`
- `v4vmm`: `docs/adr/0059-broadcast-control-surface.md`
- `musicindex-live-publisher`: `docs/architecture/broadcast-chain-boundaries.md`
- `musicindex-live-publisher`: `docs/adr/0002-nowplaying-drop-file-contract.md`
