# ADR 0012: The Display Track Carries The Play Identifier

## Status

Proposed - 2026-11-26.

Proposed 2026-11-26. The orchestrator session relayed the request of
`musicindex-live-publisher` ADR 0038, which has the status Proposed. No code
implements this record. The build starts after the acceptance of §Before
Acceptance.

- Changes: ADR 0005 items "an unknown key gives `400 invalid_display`", for
  the key `playId` only.
- Bar: crosses repositories. The publisher sends `playId`, this relay
  validates it and passes it through, and Rainbrook reads it. Without the
  record, an agent can let the relay refuse the key or use it.

Class: situational. Supersede this record when publisher ADR 0038 stops, or
when a player gets the segment identity from a different source.

## Context

A track of a display state has `artist`, `title` and `artwork` (ADR 0003),
and the optional keys `songLine` and `value` (ADR 0005) and `album`
(ADR 0006). Each other key gives `400 invalid_display`.

Publisher ADR 0038 puts a frame in the HLS stream at each segment start. A
segment is a track, or a chapter of an episode file. The frame holds only
`playId`, the play identifier of the segment. A player releases the held
display state with the same `playId`. The display state must thus carry that
identifier. An example is `ls-1795656378730-2-30665`.

The song line of ADR 0005 cannot do this. Each chapter of an episode has the
song line of the file, so a match by song line shows a chapter too early.

## Decision

### One Optional Key In The Track

A track of a display state can also have this key:

| Key | Shape | Limit |
|---|---|---|
| `playId` | A string, or `null` | 1 to 128 characters |

- The key is optional. A track with no `playId` key is valid, as before.
- `null` is valid. The publisher sends `null` for a track with no play
  identifier, for example a jingle.
- An empty string, a string over the limit, or a value that is not a string
  and not `null` gives `400 invalid_display`.
- The relay counts characters, as for `songLine`.
- The rules of ADR 0005 and ADR 0006 for the other keys do not change.

### Pass-Through

- `GET /display` and `/display/events` give the track with `playId`, without
  a change. A `null` stays `null`, and the relay adds no key.
- The relay does not use `playId`. It does not compare it with a payload, with
  an earlier display state or with the HLS stream.
- Two display states can have the same `playId`. Publisher ADR 0039 sends one
  state two times for one play.
- The body limit of 8 KiB for a display publish does not change.
- Socket.IO and the live value routes do not change.

## Invariants

These rules apply while this decision is current.

- A track with no `playId` key is valid, as before.
- The relay never changes `playId` and never adds it.
- The live value routes do not show `playId`.

## Non-Goals

- No check that `playId` has the form of the producer. The producer owns the
  form.
- No read of a display state by `playId`. A player holds the states that it
  gets from `/display/events`.

## Before Acceptance

1. **Publisher ADR 0038** is accepted. It is the only sender of the key.
2. **The operator** accepts this record.

## Verification After Implementation

Mechanical. Each item is a test in `tests/api.rs`:

- A track with a `playId` string is accepted, and `GET /display` gives it
  without a change.
- A track with `playId` `null` is accepted, and `GET /display` gives `null`.
- A track with `playId`, `album`, `songLine` and `value` is accepted.
- Each of these gives `400 invalid_display`: an empty `playId`, a `playId` of
  129 characters, and a `playId` that is a number.
- An SSE subscriber of `/display/events` receives `playId`.
- The tests of ADR 0003, ADR 0005 and ADR 0006 pass with no change.

Deployment. The review checklist has this gate:

- This relay is deployed before a publisher sends the key. An older relay
  gives `400 invalid_display` for each such state.

## Alternatives Considered

### Change ADR 0005 In Place

Not selected. ADR 0006 states that a later field gets its own record, and
ADR 0005 has the status `Implemented`.

### Accept Each Unknown Key

Rejected, as in ADR 0006. A key with any content has no limit that a test can
check.

### No `null`

Not selected. `album` has no `null` form (ADR 0006). But the frame of
publisher ADR 0038 has `playId` `null`, and the publisher sends the same
value in the two places. A refused `null` would remove the display state of
each jingle.

## Consequences

Positive:

- A player on HLS can show each segment when the listener hears it.

Negative and risks:

- The display state grows by about 140 bytes at most.
- An older relay refuses the key. The deployment order matters.
- The track now has four optional keys from three records. A fifth key is a
  cause to write one current record for the display track.

## References

- `docs/adr/0003-display-state-and-artwork.md`
- `docs/adr/0005-display-song-line-and-value.md`
- `docs/adr/0006-display-album.md`
- `musicindex-live-publisher`: `docs/adr/0038-the-hls-stream-marks-each-segment.md`
- `musicindex-live-publisher`: `docs/adr/0039-the-display-state-pairs-with-its-value-block.md`
