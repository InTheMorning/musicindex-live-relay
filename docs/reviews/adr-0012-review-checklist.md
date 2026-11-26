# ADR 0012 Display Play Identifier: Review Checklist

Status: open - 2026-11-26. The deployment item remains. When it closes, this
checklist is the named artifact for the `Implemented` status of ADR 0012.

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

- [ ] The operator deploys this relay on `api.musicindex.org` before a
  publisher sends `playId`. Record the deployed commit here.
