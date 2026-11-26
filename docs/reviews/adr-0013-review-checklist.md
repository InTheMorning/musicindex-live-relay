# ADR 0013 Optional Keys Of The Display Track: Review Checklist

Status: closed - 2026-11-26. Each item passes. This checklist is the named
artifact for the `Implemented` status of ADR 0013.

## Invariants Of ADR 0013

- [x] A track with only `artist`, `title` and `artwork` is valid. The tests
  of ADR 0003 pass with no change.
- [x] The relay never changes an optional key and never adds one. Each
  pass-through test compares the full state.
- [x] The live value routes do not show an optional key. A reader confirmed
  that only `validate_display` and its two check functions name `pubDate`
  and `funding`.
- [x] Each optional key has a shape and a limit, and a test for each.

## The Restated Rules

- [x] The tests of `songLine`, `value`, `album` and `playId` pass with no
  change to their assertions. The test of a track with each optional key
  got `pubDate` and `funding` in its body.

## Verification Of `pubDate`

- [x] `2026-11-12` is accepted and read without a change. Test:
  `a_track_with_pub_date_is_accepted_and_passed_through`.
- [x] `pubDate` with the other optional keys is accepted. Test:
  `a_track_with_pub_date_and_other_optional_keys_is_accepted`.
- [x] `2024-02-29` is accepted. Test: `a_leap_day_pub_date_is_accepted`.
- [x] Each incorrect form gives `400 invalid_display`. Test:
  `invalid_pub_date_gives_400`. Unit test:
  `calendar_day_requires_a_real_ascii_date`.
- [x] An SSE subscriber receives `pubDate`. Test:
  `a_subscriber_receives_pub_date`.

## Verification Of `funding`

- [x] Two entries, one with no `label`, are accepted and read without a
  change, and an SSE subscriber receives them. Test:
  `funding_is_passed_through_and_sent_to_subscribers`.
- [x] Three entries at the limits of `url` and `label` are accepted. Test:
  `funding_accepts_three_entries_at_length_limits`.
- [x] Each incorrect form gives `400 invalid_display`. Test:
  `invalid_funding_gives_400`. Unit test:
  `funding_rejects_newlines_and_tabs_in_urls`.
- [x] `artwork.url` keeps its rule. The tests of `artwork.url` pass with no
  change.

## Documents

- [x] `README.md` and `docs/interoperability.md` name `pubDate` and
  `funding`, their shapes and the deployment order.
- [x] ADR 0005, ADR 0006 and ADR 0012 have the status
  `Superseded by ADR 0013` and are in `docs/archive/adr/`.

## Deployment

- [x] The operator deploys this relay on `api.musicindex.org` before a
  publisher sends `pubDate` or `funding`.

## Deployment Result

The operator deployed the relay on `api.musicindex.org` on 2026-11-26.

- `GET https://api.musicindex.org/v1/liveitems/info` gave
  `{"version":"0.1.0","git_revision":"1dec0b3","built_at":"2026-11-26T19:27:56Z"}`
  on 2026-11-26. The relay session made this read.
- `GET /v1/liveitems/Nx7Pcae5FVqzwZtMDZiXFw/display` gave a track with
  `funding`, one entry with `label` and `url`, and with `playId`
  `ls-1795725140990-5`, on 2026-11-26. The relay session made this read.
- The publisher session reported that the same route gave `pubDate`
  `2026-11-12` for the play `ls-1795725060190-4-30665`, and that the
  publisher got no `400`. The record is publisher station log step 62,
  publisher commit `0a884e1`. The relay session did not make this check.
