# ADR 0013: The Optional Keys Of The Display Track

## Status

Accepted - 2026-11-26.

Accepted 2026-11-26. The operator accepted this record on 2026-11-26, as
written, and then publisher ADR 0042 and publisher ADR 0043 (publisher
commit `2e517df`). The two task packets are done. This record gets the
status `Implemented` when the deployment gate of
`docs/reviews/adr-0013-review-checklist.md` closes.

Changed 2026-11-26, after the operator acceptance and before the status
`Accepted`. The table gets the key `funding` for publisher ADR 0043, by the
rule of §The Next Key. The operator chose on 2026-11-26 that the display
state carries the funding links of the track. The orchestrator session
relayed that choice.

Proposed 2026-11-26. The orchestrator session relayed the request of
`musicindex-live-publisher` ADR 0042 for the key `pubDate`. The operator
chose on 2026-11-26 that an episode sends its publication date in the display
state only.

- Supersedes: ADR 0005, ADR 0006 and ADR 0012. The three records are in
  `docs/archive/adr/`.
- Bar: crosses repositories. The publisher sends the keys, this relay
  validates them and passes them through, and Rainbrook and the HLS tagger
  read them. Without the record, an agent can let the relay refuse a key, or
  accept a key with no limit.

Class: situational. Supersede this record when the display state gets its
track from a different source, or when the track gets a different shape.

## Context

A track of a display state has the keys `artist`, `title` and `artwork`
(ADR 0003). Three records added optional keys:

- ADR 0005 added `songLine` and `value`.
- ADR 0006 added `album`.
- ADR 0012 added `playId`.

ADR 0012 states that a fifth optional key is a cause to write one current
record for the display track. Publisher ADR 0042 asks for that fifth key,
`pubDate`. It is the publication day of an episode. `station-import` of the
publisher writes the `pubDate` of the feed item into the file as the UTC day.
A player shows the day with the episode.

Publisher ADR 0043 asks for a sixth key, `funding`. It is the list of the
funding links of the track, from the tag `podcast:funding` of the feed. A
player shows the links, and a listener opens them.

This record thus does three things:

- It restates each live rule of ADR 0005, ADR 0006 and ADR 0012, with no
  change.
- It adds the key `pubDate`.
- It adds the key `funding`.

## Decision

### The Optional Keys

A track of a display state has `artist`, `title` and `artwork`, as ADR 0003
states them. It can also have these keys:

| Key | Shape | Limit | `null` |
|---|---|---|---|
| `songLine` | A string: the line that `butt` sends as the ICY `StreamTitle` | 1 to 1,024 characters | Not valid |
| `value` | `{"eventGuid": "…", "blockGuid": "…"}`, two strings and no other key | Each string 1 to 128 characters | Not valid |
| `album` | A string: the album of the track | 1 to 1,024 characters | Not valid |
| `playId` | A string: the play identifier of the segment | 1 to 128 characters | Valid |
| `pubDate` | A string: a calendar day in the form `YYYY-MM-DD` | Exactly 10 characters | Not valid |
| `funding` | An array of entries: the funding links of the track | 1 to 3 entries | Not valid |

- Each key is optional. A track with none of these keys is valid.
- Each other key gives `400 invalid_display`.
- A key with a different shape, an empty string or a string over its limit
  gives `400 invalid_display`.
- The relay counts characters, not bytes.
- A sender that has no value for a key sends no key. `playId` is the one
  exception: the publisher sends `null` for a track with no play identifier,
  for example a jingle.

### The Form Of `pubDate`

- `pubDate` is four digits, a hyphen, two digits, a hyphen and two digits.
  Each digit is an ASCII digit. An example is `2026-11-12`.
- The day must exist in the Gregorian calendar. `2026-02-30` and
  `2026-13-01` give `400 invalid_display`.
- A time, a time zone or text after the day gives `400 invalid_display`.
- The relay does not compare the day with its clock. A day in the future is
  valid.
- The relay accepts `pubDate` on each track. It does not know if a track is
  an episode. The publisher sends the key only for an episode.

### The Form Of `funding`

Each entry of `funding` is an object with these keys and no other key:

| Key | Shape | Limit |
|---|---|---|
| `url` | A string, necessary | 1 to 512 characters |
| `label` | A string, optional | 1 to 128 characters |

- `null` and an empty array give `400 invalid_display`. A sender with no
  link sends no key.
- A fourth entry gives `400 invalid_display`.
- The scheme of `url` is `http` or `https`. The relay ignores the case of
  the scheme, as for `artwork.url`.
- `url` has a host: after `://`, one character or more comes before the
  first `/`, `?` or `#`. `https:///support` gives `400 invalid_display`.
- `url` has no white space and no control character.
- `label` has no white space at its start or at its end. An entry with no
  label text has no `label` key.
- Two entries with the same `url` give `400 invalid_display`. The relay
  compares the two strings character by character.
- One incorrect entry makes the full display state invalid. The relay does
  not remove an entry.
- The relay does not request a `url`, and it does not compare a `url` with a
  feed.

### Pass-Through

- `GET /display` and `/display/events` give the track with its keys, without
  a change. A `null` `playId` stays `null`, and the relay adds no key.
- The relay does not use an optional key. It does not compare `value` with a
  published payload, and it does not check that the block exists. It does not
  compare `playId` with a payload, with an earlier display state or with the
  HLS stream.
- Two display states can have the same `playId`. Publisher ADR 0039 sends one
  state two times for one play.
- The body limit of 8 KiB for a display publish does not change.
- Socket.IO and the live value routes do not change. The live value payload
  does not get `pubDate` or `funding`.
- Three entries of `funding` at their limits use about 2 KiB of ASCII text,
  and more with other characters. The publisher owns the fit in the body
  limit (publisher ADR 0043 §The Publisher).

### The Next Key

A new optional key is a change to the table of this record. It needs no new
record when all of these conditions are true:

- The key has a shape and a limit that a test can check.
- The relay passes the key through and does not use it.

The change gets a dated sentence in §Status and a `- Revised:` line. A key
that the relay uses needs a new record.

## Invariants

These rules apply while this decision is current.

- A track with only `artist`, `title` and `artwork` is valid.
- The relay never changes an optional key and never adds one.
- The live value routes do not show an optional key.
- Each optional key has a shape and a limit, and a test for each.

## Non-Goals

- No check that `value` names a real block.
- No check that `playId` has the form of the producer.
- No check that `album`, `pubDate` or `funding` agrees with a payload or a
  feed.
- No request to a `url` of `funding`.
- No read of a display state by `playId` or by `pubDate`.
- No key with free content, such as `extra`. Such a key has no limit that a
  test can check.

## Before Acceptance

1. **Publisher ADR 0042** is accepted. It is the only sender of `pubDate`.
2. **The operator** accepts this record.

The key `funding` has its own condition for the build: publisher ADR 0043 is
accepted. It is the only sender of `funding`. That condition does not hold
the status of this record or the build of `pubDate`.

## Changes At Acceptance

All of these are in one commit:

- ADR 0005, ADR 0006 and ADR 0012 get the status
  `Superseded by ADR 0013` and move to `docs/archive/adr/`.
- `AGENTS.md`, `docs/README.md`, `README.md` and `docs/interoperability.md`
  name this record as the owner of the optional keys.
- The comments in `src/lib.rs` name this record.

## Verification After Implementation

Mechanical. Each item is a test in `tests/api.rs`.

The tests of ADR 0005, ADR 0006 and ADR 0012 stay, with no change to their
assertions. They prove the rules that this record restates.

For `pubDate`:

- A track with `pubDate` `2026-11-12` is accepted, and `GET /display` gives
  it without a change.
- A track with `pubDate`, `playId`, `album`, `songLine` and `value` is
  accepted.
- A track with `pubDate` `2024-02-29` is accepted.
- Each of these gives `400 invalid_display`:
  - an empty `pubDate`, and a `pubDate` that is `null` or a number,
  - `2026-02-30`, `2026-13-01` and `2026-00-10`,
  - `2026-1-5`, `20261112` and `2026/11/12`,
  - `2026-11-12T00:00:00Z`.
- An SSE subscriber of `/display/events` receives `pubDate`.

For `funding`:

- A track with two entries, one with `label` and one with no `label`, is
  accepted, and `GET /display` gives the array without a change.
- A track with three entries is accepted.
- A track with `funding`, `pubDate`, `playId`, `album`, `songLine` and
  `value` is accepted.
- A `url` of 512 characters and a `label` of 128 characters are accepted.
- Each of these gives `400 invalid_display`:
  - `funding` that is `null`, an empty array, an object or a string,
  - four entries,
  - an entry that is not an object, an entry with no `url`, and an entry
    with a third key,
  - a `url` with the scheme `ftp`, `https:///support`, a `url` of 513
    characters, an empty `url` and a `url` with a space,
  - an empty `label`, a `label` of 129 characters, a `label` with a space at
    its end, and a `label` that is `null`,
  - two entries with the same `url`.
- An SSE subscriber of `/display/events` receives `funding`.

Deployment. The review checklist has this gate:

- This relay is deployed before a publisher sends `pubDate` or `funding`. An
  older relay gives `400 invalid_display` for each such state.

## Alternatives Considered

### A Small Record That Changes ADR 0005

Not selected. That is the form of ADR 0006 and ADR 0012. An agent must then
read four records to know the keys of one track, and each new key adds a
record. ADR 0012 named the fifth key as the time to stop.

### A String With A Limit And No Form Check

Not selected. It is the form of `album`, and it needs less code. But each
reader must then check the form, and the relay is the one place that all
readers share. The crate `time` is already a dependency and checks the day.

### The Full Date And Time Of The Feed Item

Not selected. The publisher has only the day in the file. A player shows a
day, not a time.

### `null` For A Track With No Date

Not selected. `playId` has `null` because the HLS frame of publisher
ADR 0038 has the same value. `pubDate` has no such second place. No key is
the form of `album` and `songLine`.

## Consequences

Positive:

- A player shows the publication day of an episode.
- One record owns the optional keys of the track. The reading path gets
  three records shorter.
- A sixth pass-through key needs a table row and tests, not a record.

Negative and risks:

- The display state grows by about 25 bytes for `pubDate`. For `funding`
  it grows by about 2 KiB of ASCII text at most.
- An older relay refuses `pubDate` and `funding`. The deployment order
  matters.
- A listener opens a `url` of `funding`. The text comes from a feed that a
  third party writes. The relay checks only the scheme, the host and the
  characters. A player must show the host before it opens the link, or
  accept that risk.
- The relay is stricter for `pubDate` than for `album`. A publisher that
  sends a day that does not exist loses the full display state of that
  track, not only the date.
- Three records move. Each link to them in the documents changes in the same
  commit.

## References

- `docs/adr/0003-display-state-and-artwork.md`
- `docs/archive/adr/0005-display-song-line-and-value.md`
- `docs/archive/adr/0006-display-album.md`
- `docs/archive/adr/0012-display-play-id.md`
- `musicindex-live-publisher`: `docs/adr/0042-an-episode-sends-its-publication-date.md`
- `musicindex-live-publisher`: `docs/adr/0043-the-display-state-carries-the-funding-links.md`
- `musicindex-live-publisher`: `docs/adr/0038-the-hls-stream-marks-each-segment.md`
- `musicindex-live-publisher`: `docs/adr/0039-the-display-state-pairs-with-its-value-block.md`
