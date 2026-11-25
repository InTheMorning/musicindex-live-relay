# ADR 0011 Listener Images: Review Checklist

Status: closed - 2026-11-25. Each item passes. This checklist is the named
artifact for the `Implemented` status of ADR 0011.

ADR 0011 has no visual item. The app check of the payload image belongs to
`musicindex-live-publisher` ADR 0034 §Before Acceptance.

## Invariants Of ADR 0011

- [x] A display publish and an upload keep each listener image, in the limit.
  Test: `pending_listener_images_survive_display_publishes_until_the_timeline_moves_on`.
- [x] An upload or a display publish never removes the image of the present
  display state. Tests: the two eviction tests of this checklist, and the
  ADR 0003 tests with the limit 2.
- [x] An event never holds more than `MAX_EVENT_IMAGES` images. Test: the
  ADR 0003 test of uploads with no display publish, with the limit 2.
- [x] The relay never changes a payload and never refuses it because of its
  `image`. Test:
  `a_payload_that_names_an_image_the_event_does_not_hold_is_accepted`.

## Verification Of ADR 0011

- [x] Image A stays while its update is pending and while it is the present
  listener value. The next display publish after a different listener value
  removes it. Test:
  `pending_listener_images_survive_display_publishes_until_the_timeline_moves_on`.
- [x] An `image` with a different event identifier, an incorrect hash or text
  after the hash keeps no image. Unit test:
  `payload_artwork_sha256_accepts_only_the_event_image_suffix`.
- [x] The order of removal at the limit. Tests:
  `eviction_prefers_unreferenced_then_listener_then_previous_display_images`
  and `listener_image_is_evicted_before_the_earlier_display_image`.
- [x] `MAX_EVENT_IMAGES` has the default 8, reads the environment and refuses
  0 and 1. Unit test:
  `max_event_images_has_a_default_reads_the_environment_and_rejects_low_values`.
- [x] A lease expiry removes a listener image. Test:
  `a_lease_expiry_with_a_snapshot_removes_the_images`.

## Code

- [x] No function holds the `inner` lock and the `display` lock at the same
  time. A reader confirmed this in `publish_display` and `upload_artwork`.
- [x] `payload_artwork_sha256` uses `is_sha256_hex`. It has no second copy of
  the hash check.
- [x] `Cargo.toml` has no new dependency, and no route changed.
- [x] An independent ChatGPT review found no deadlock and no acceptance
  criterion without a test. The task packet records its two results.

## Documents

- [x] `README.md` and `docs/interoperability.md` state the rule and the
  limit.
- [x] ADR 0003 has the line `- Changed by: ADR 0011`.
- [x] `AGENTS.md` and `docs/README.md` name ADR 0011.

## Deployment

The relay on `api.musicindex.org` does not have this change until the
operator deploys it. That is not a gate of ADR 0011. Publisher ADR 0034
needs the deployed relay before its app check.
