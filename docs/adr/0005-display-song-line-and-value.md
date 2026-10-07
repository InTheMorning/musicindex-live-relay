# ADR 0005: The Display State Carries The Song Line And The Value Identity

## Status

Implemented - 2026-10-06.

Implemented 2026-10-06: task 001 is done, and the relay is deployed before
the publisher. The named artifact is
`docs/reviews/adr-0005-review-checklist.md`, with no open item.

Accepted - 2026-10-06.

Accepted 2026-10-06 by the operator. Item 1 of §Before Acceptance is done:
publisher ADR 0012 is accepted. Item 2, the deployment order, is a gate of
the review checklist.

Proposed 2026-10-06.

Class: situational. Supersede this record when the tagger of
`musicindex-live-publisher` ADR 0009 gets its pairing from a different source.

## Context

`musicindex-live-publisher` ADR 0009 puts a MusicIndex frame into each HLS
segment, at the ICY title. A tagger on the stream host selects a display state
when the ICY title names it. The frame then names the live value block of
that track, so a player pays the block of the track that the listener hears.

Two facts are missing from the display state today:

- **The exact song line.** The tagger compares the ICY title with
  `artist - title`. The format rules of `now-playing.txt` can make the two
  differ, for example the hyphen removal. An exact copy of the line that
  `butt` sends removes the text rules.
- **The value identity.** The live value payload has `eventGuid` and
  `blockGuid`. No component can pair a display state with its payload.

The operator accepted the pairing of ADR 0009 on 2026-10-06. The publisher
side is `musicindex-live-publisher` ADR 0012.

ADR 0003 accepts a track with exactly the keys `artist`, `title` and
`artwork`, and gives `400 invalid_display` for any other key.

## Decision

### Two Optional Keys In The Track

A track of a display state can also have these keys:

| Key | Shape | Limit |
|---|---|---|
| `songLine` | A string: the line that `butt` sends as the ICY `StreamTitle` | 1 to 1,024 characters |
| `value` | `{"eventGuid": "…", "blockGuid": "…"}`, two strings and no other key | Each string 1 to 128 characters |

- Each key is optional. A track with neither key is valid, as before.
- A key with a different shape, an empty string, a string over its limit, or
  an unknown key gives `400 invalid_display`.
- The relay does not compare `value` with a published payload. It does not
  check that the block exists. The broadcaster owns that pairing.

### Pass-Through

- `GET /display` and `/display/events` give the track with its keys, without
  a change.
- The body limit of 8 KiB for a display publish does not change.
- Socket.IO and the live value routes do not change.

### Timing

The display routes are instant (ADR 0004). A consumer that waits for an
in-band key gets each state before its ICY title.

## Invariants

These rules apply while this decision is current.

- A track with only `artist`, `title` and `artwork` is valid, as before.
- The relay never changes `songLine` or `value`.
- The live value routes do not show `songLine` or `value`.

## Non-Goals

- No check that `value` names a real block.
- No use of `songLine` by the relay.

## Before Acceptance

1. **Publisher ADR 0012** is accepted. It is the only sender of these keys.
2. **The deployment order.** This relay is deployed before a publisher sends
   the keys. An older relay gives `400 invalid_display` for each such state.

## Verification After Implementation

Mechanical. Each item is a test in `tests/api.rs`:

- A track with `songLine` and `value` is accepted, and `GET /display` gives
  both keys without a change.
- A track with only `songLine`, and a track with only `value`, are accepted.
- Each of these gives `400 invalid_display`:
  - an empty `songLine`, and a `songLine` of 1,025 characters,
  - a `value` with a third key, and a `value` with an empty `blockGuid`,
  - an unknown track key.
- An SSE subscriber of `/display/events` receives both keys.
- The tests of ADR 0003 pass with no change.

## Alternatives Considered

### A Separate Pairing Route

Rejected. A separate route needs its own replay buffer and its own order with
the display state. A key in the track arrives in the same update.

### The Relay Pairs By Content

Rejected. The relay would compare titles of the display state and the
payload. The payload has no artist, and this relay passes payloads through
without interpretation (`AGENTS.md` §1).

## Consequences

Positive:

- The tagger matches the ICY title by equality.
- The tagger can name the exact block in the HLS frame.

Negative and risks:

- The display state grows by up to about 1.3 KB.
- An older relay refuses the new keys. The deployment order matters.

## References

- `docs/adr/0003-display-state-and-artwork.md`
- `docs/adr/0004-listener-timeline-delay.md`
- `musicindex-live-publisher`: `docs/adr/0009-hls-track-metadata.md`
- `musicindex-live-publisher`: `docs/adr/0012-pair-display-state-with-value-block.md`
