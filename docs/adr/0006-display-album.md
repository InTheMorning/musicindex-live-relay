# ADR 0006: The Display Track Carries The Album

## Status

Accepted - 2026-10-06.

Accepted 2026-10-06 by the operator. Item 1 of §Before Acceptance is done:
publisher ADR 0013 is accepted. Item 2, the deployment order, is a gate of
the review checklist.

Proposed 2026-10-06.

Class: situational. Supersede this record when the display state gets its
track text from a different source.

## Context

A listener app shows the track from the display state of ADR 0003. The track
has `artist`, `title`, `artwork`, and the optional keys `songLine` and
`value` of ADR 0005. It has no album.

The album is on the wire in one place only. The live value payload of a V4V
track has `podcastName` and `line[0]` (`musicindex-live-publisher` ADR 0010).
That payload exists only for a V4V track. A track that pays nobody has no
payload, so an app that reads the album from the payload shows no album for
that track.

The producer reads the album from the Mixxx history row for each track
(`musicindex-live-publisher` ADR 0010). The publisher side of this record is
`musicindex-live-publisher` ADR 0013.

ADR 0005 gives `400 invalid_display` for each track key that it does not
name.

## Decision

### One Optional Key In The Track

A track of a display state can also have this key:

| Key | Shape | Limit |
|---|---|---|
| `album` | A string: the album of the track | 1 to 1,024 characters |

- The key is optional. A track with no album has no `album` key. It does not
  have `null` or an empty string.
- An empty string, a string over the limit, or a value that is not a string
  gives `400 invalid_display`.
- The rules of ADR 0005 for the other keys do not change.

### Pass-Through

- `GET /display` and `/display/events` give the track with `album`, without a
  change.
- The body limit of 8 KiB for a display publish does not change.
- Socket.IO and the live value routes do not change.

## Invariants

These rules apply while this decision is current.

- A track with no `album` key is valid, as before.
- The relay never changes `album`.
- The live value routes do not show `album`.

## Non-Goals

- No album in the live value payload. ADR 0010 of the publisher owns
  `podcastName` and `line`.
- No check that the album agrees with the payload.

## Before Acceptance

1. **Publisher ADR 0013** is accepted. It is the only sender of the key.
2. **The deployment order.** This relay is deployed before a publisher sends
   the key. An older relay gives `400 invalid_display` for each such state.

## Verification After Implementation

Mechanical. Each item is a test in `tests/api.rs`:

- A track with `album` is accepted, and `GET /display` gives it without a
  change.
- A track with `album`, `songLine` and `value` is accepted.
- Each of these gives `400 invalid_display`: an empty `album`, an `album` of
  1,025 characters, and an `album` that is not a string.
- An SSE subscriber of `/display/events` receives `album`.
- The tests of ADR 0003 and ADR 0005 pass with no change.

## Alternatives Considered

### The App Reads The Album From The Live Value Payload

Rejected as the only source. It works only for a V4V track, and the app must
read a second stream. The app can still do it. This record does not prevent
it.

### A Generic Key For More Track Fields

Rejected. A key such as `extra` with any content has no limit that a test can
check. This relay validates each key of the display state (ADR 0003). A later
field gets its own record.

## Consequences

Positive:

- An app shows the album for each track, also a track that pays nobody.
- The HLS tagger of publisher ADR 0009 can copy the album into its frame.

Negative and risks:

- The display state grows by about 1 KB at most.
- An older relay refuses the key. The deployment order matters.

## References

- `docs/adr/0003-display-state-and-artwork.md`
- `docs/adr/0005-display-song-line-and-value.md`
- `musicindex-live-publisher`: `docs/adr/0010-live-value-payload-reference-shape.md`
- `musicindex-live-publisher`: `docs/adr/0013-display-album.md`
