# ADR 0011: The Relay Keeps Each Image That The Listener Timeline Names

## Status

Implemented - 2026-11-25.

Implemented 2026-11-25. The named artifact is
`docs/reviews/adr-0011-review-checklist.md`, which has no open gates.

Accepted 2026-11-25. The operator chose the relay copy of the picture as the
payload image fallback on 2026-11-25 (publisher ADR 0034). The orchestrator
session relayed the task and left the design of the retention rule to this
repository. The operator can change the limit of §The Limit and the rule of
§The Lease End with an amendment.

Proposed 2026-11-25.

- Changes: ADR 0003 items "the relay keeps the image of the present display
  state and the image of the most recent earlier state with a relay image,
  and removes every other image", the limit of two images for each event, the
  invariant on the display memory, and the check "A third image removes the
  first one".
- Bar: crosses repositories. `musicindex-live-publisher` ADR 0034 puts the
  URL of a relay image in the payload. The publisher and the retention rule
  of this relay must agree, or a podcast app gets a URL that gives `404`.

Class: situational. Supersede this record when publisher ADR 0034 stops, when
ADR 0004 stops, or when the payload image comes from a different host.

## Context

ADR 0003 gives a reserved event an image store. The event keeps two images at
most. One is the image of the present display state. The other is the image
of the most recent earlier state with a relay image. The display routes are
on the instant timeline (ADR 0004).

Publisher ADR 0034 uses the same image for podcast apps. The `image` field of
a payload can be the URL
`{endpoint}/v1/liveitems/{event_id}/artwork/{sha256}`. A payload reaches
Socket.IO and `GET /remoteValue` after the listener delay of ADR 0004, which
can be 300 seconds.

The two rules do not agree. Two tracks with pictures can start during the
delay. The relay then removes the image of the first track before the apps
get the payload that names it. The app loads the URL and gets
`404 artwork_not_found`.

Terms:

- A **listener image** of an event is an image that the listener timeline
  of that event names. The present listener value can name it, or a pending
  listener update. This is a project term.
- A **display image** of an event is one of the two images of ADR 0003. One
  is the image of the present display state. The other is the image of the
  most recent earlier state with a relay image. This is a project term.

## Decision

### How A Payload Names An Image

A live value names an image of the event when all of these conditions are
true:

- The live value is an object with a top-level `image` key that is a string.
- The string ends with `/v1/liveitems/{event_id}/artwork/{sha256}`.
- `{event_id}` is the identifier of the event that holds the live value.
- `{sha256}` is 64 lowercase hexadecimal characters.

The relay does not compare the start of the URL, because it does not know its
public address. The relay only reads the `image` key. It does not change the
payload, and a payload that names an image that the relay does not hold is
accepted as before (AGENTS.md §1).

### The Retention Rule

After a display publish, the event keeps these images and removes each other
image:

- each display image,
- each listener image.

The relay reads the listener images at the time of the display publish and of
an upload. It does not remove an image at the time that a pending update
reaches the listener timeline. An image that the listener timeline stopped
naming thus stays until the next display publish. §The Limit limits the
count of such images.

The relay reads the listener images, and then it changes the store. It does
not hold the lock of the live value during that change. A payload that the
relay accepts during a display publish or an upload thus counts as a payload
after it. The rule keeps an image from the time that the relay accepted the
payload that names it. Before that time, only a display state keeps the
image.

### The Limit

- An event holds at most `MAX_EVENT_IMAGES` images. The default is 8. The
  lowest accepted value is 2, for the two display images.
- An upload of a new image to an event that holds `MAX_EVENT_IMAGES` images
  first removes one image. The relay selects the first group that has an
  image, and in that group the earliest upload:
  1. an image that is not a display image and not a listener image,
  2. a listener image that is not a display image,
  3. the image of the earlier display state.
- An upload never removes the image of the present display state.

The display path thus comes before the payload image. When the limit is too
low for the delay, a podcast app can get `404` for an earlier image. The
display publish of the present track always succeeds.

### The Lease End

ADR 0003 does not change here. When the lease of an event ends, the relay
removes each image of the event immediately, also each listener image. A delete
of the event does the same.

### No Change To The Transports

No route, no status code and no body changes. `PUT` and
`GET /v1/liveitems/{event_id}/artwork/{sha256}` stay as ADR 0003 states them.
The image store stays in memory only.

## Invariants

These rules apply while this decision is current.

- After a display publish or an upload, the event holds each listener image
  that it held before, unless the limit of §The Limit removed it.
- An upload or a display publish never removes the image of the present
  display state.
- An event never holds more than `MAX_EVENT_IMAGES` images.
- The display memory is at most `MAX_RESERVED_ITEMS` multiplied by
  `MAX_EVENT_IMAGES` images of `ARTWORK_MAX_BYTES`. With the defaults that is
  100 × 8 × 512 KiB, which is 400 MiB.
- The relay never changes a payload, and it never refuses a payload because
  of its `image`.

## Verification After Implementation

Mechanical, each with a test. A test with a delay uses the injected clock.

- A payload with a delay names image A. Two subsequent tracks each upload an
  image and publish a display state. `GET` of image A gives `200` while the
  update is pending, and also after the update is the present listener value.
- After a different payload is the present listener value, the next display
  publish removes image A, and `GET` gives `404 artwork_not_found`.
- An `image` with a different event identifier, with a hash that is not 64
  lowercase hexadecimal characters, or with text after the hash keeps no
  image.
- An upload to an event that holds `MAX_EVENT_IMAGES` images removes an image
  of group 1 before an image of group 2, and an image of group 2 before the
  image of the earlier display state.
- An event never holds more than `MAX_EVENT_IMAGES` images after uploads with
  no display publish.
- `MAX_EVENT_IMAGES` has the default 8 and reads the environment. A value
  below 2 is a configuration error.
- A lease expiry removes a listener image.

No visual check. Each rule is in the store, and no person can see the store.
The app check of the full fallback belongs to publisher ADR 0034 §Before
Acceptance.

## Alternatives Considered

### Keep Each Image For `MAX_LISTENER_DELAY_SECS` After Its Display State

Not selected. It needs no read of the payload. But the present listener value
can name an image for longer than the delay, for example while tracks with no
payload play. A new Socket.IO client then gets a payload with a URL that
gives `404`.

### A Count Of References That A Publish And A Release Change

Not selected. A publish, a release, a full pending list, a lease expiry and a
delete must then each change the count correctly. The selected rule reads the
listener timeline when it removes an image, so no count can be wrong.

### Refuse An Upload At The Limit

Rejected. The display path is the first user of the store. A refused upload
breaks the picture of the present track for the private app.

### Keep The Listener Images After The Lease End

Not selected. The relay must then remove images when a pending update reaches
the listener timeline, with a second lock in the release sweep. After a lease
end, the broadcaster is gone and the stream has usually stopped.

## Consequences

Positive:

- A podcast app can load the relay URL of a payload for as long as the
  listener timeline names it, in the limit of §The Limit.
- The display path does not change for its clients.

Negative and risks:

- The highest display memory is four times that of ADR 0003, with the
  defaults.
- After a lease end, the listener timeline can name a removed image until
  its `{}` arrives. That time is at most the delay of the last publish.
- A relay restart removes each image, as before. A payload from before the
  restart is not restored (ADR 0001), so no payload names a removed image
  after a restart.
- The relay reads one key of the payload for the first time. It reads the
  key only to keep an image. A change of the payload shape in the publisher
  that moves `image` stops the retention, and the tests of the publisher do
  not find that.

## Changes At Acceptance

- ADR 0003 gets the line `- Changed by: ADR 0011`.
- `README.md`, `docs/interoperability.md` and `AGENTS.md` state the new rule
  and the new limit, in the same commit as the code.

## References

- `docs/adr/0003-display-state-and-artwork.md`
- `docs/adr/0004-listener-timeline-delay.md`
- `musicindex-live-publisher`:
  `docs/adr/0034-the-payload-image-falls-back-to-the-relay-copy-of-the-picture.md`
