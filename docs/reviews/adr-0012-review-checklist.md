# ADR 0012 Display Play Identifier: Review Checklist

Status: closed - 2026-11-26. Each item passes. This checklist is the named
artifact for the `Implemented` status of ADR 0012.

## Invariants Of ADR 0012

- [x] A track with no `playId` key is valid, as before. The tests of
  ADR 0003, ADR 0005 and ADR 0006 pass with no change.
- [x] The relay never changes `playId` and never adds it. Tests:
  `a_track_with_play_id_is_accepted` and
  `a_track_with_null_play_id_is_accepted` compare the full state.
- [x] The live value routes do not show `playId`. A reader confirmed that
  only `validate_display` names the key.

## Verification Of ADR 0012

- [x] A `playId` string is accepted and read without a change. Test:
  `a_track_with_play_id_is_accepted`.
- [x] `playId` `null` is accepted and read as `null`. Test:
  `a_track_with_null_play_id_is_accepted`.
- [x] `playId` with `album`, `songLine` and `value` is accepted. Test:
  `a_track_with_play_id_and_other_optional_keys_is_accepted`.
- [x] An empty `playId`, 129 characters and a number each give
  `400 invalid_display`. Test: `invalid_play_id_gives_400`. The test
  `a_play_id_of_128_characters_is_accepted` proves the limit.
- [x] An SSE subscriber receives `playId`. Test:
  `a_subscriber_receives_play_id`.

## Documents

- [x] `README.md` and `docs/interoperability.md` name `playId`, its shape and
  the deployment order.
- [x] ADR 0005 has the line `- Changed by: ADR 0012`.

## Deployment

- [x] The operator deploys this relay on `api.musicindex.org` before a
  publisher sends `playId`.

## Deployment Result

The operator deployed the relay on `api.musicindex.org` on 2026-11-26.

- `GET https://api.musicindex.org/v1/liveitems/info` gave
  `{"version":"0.1.0","git_revision":"d1dd6b3","built_at":"2026-11-26T05:04:18Z"}`
  on 2026-11-26. The relay session made this read.
- The publisher session reported that the publisher on the test station sent
  display states with `playId` at 2026-11-26 05:07:06 UTC, and that the relay
  accepted each one. The record is publisher station log step 52, publisher
  commit `6819fb1`. The relay session did not make this check.
