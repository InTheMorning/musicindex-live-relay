# ADR 0003 Implementation Review

## Status

Pass - 2026-10-06. Each invariant of ADR 0003 has a test. No gate is open. This review is the named artifact for the `Implemented` status of ADR 0003.

## Reviewed Artifacts

- `docs/adr/0003-display-state-and-artwork.md`
- `docs/plans/adr-0003-display-state-phase-plan.md`
- `docs/tasks/display-state-task-001-state-and-routes.md` with the §Review Result
- `docs/tasks/display-state-task-002-artwork-store.md` with the §Review Result
- `docs/interoperability.md` §The display state and the images are in memory only
- `README.md` §Display State and all subsections
- `src/lib.rs`: `DisplayInner`, `LiveEventState.display`, `publish_display`, `display_state`, `display_events`, `upload_artwork`, `artwork`, `subscribe_display`, `clear_display_after_lease`, `expire_leases`
- `tests/api.rs`: display and artwork test modules

## Test Commands

All four commands are Green on 2026-10-06:

- `cargo fmt -- --check`
- `cargo build`
- `cargo test`: 47 unit tests, 116 API tests and 2 startup tests
- `cargo clippy --all-targets -- -D warnings`

## Invariants

The tests are in `tests/api.rs`, module `reserved::display` and `reserved::display::artwork`, unless the table names another place.

| Invariant | Result | Guard |
|---|---|---|
| Only a reserved event has a display state or images. | Pass | `an_ephemeral_event_gives_409_on_each_route`. Any display route returns `409 event_not_reserved` for an ephemeral event. |
| No display state or image reaches disk. | Pass | `a_restart_gives_a_null_state` and `a_restart_holds_no_image`. After a restart, both routes read `{"track": null}` and no image. The `DisplayInner` holds the state and images in memory only. The store holds no payload. |
| The display memory is at most the reserved event limit multiplied by two images of `ARTWORK_MAX_BYTES`. | Pass | `MAX_EVENT_IMAGES` constant is 2 (src/lib.rs:73). `store_image` at src/lib.rs:1593 enforces this when an event holds two images and a third arrives. `remove_images` at src/lib.rs:1656 clears images on lease end. `AppConfig` at src/lib.rs:144 holds `artwork_max_bytes` configurable from environment. |
| Every write route has a body limit. | Pass | Routes at src/lib.rs:2139–2154. `POST /v1/liveitems/{event_id}/display` has `DISPLAY_BODY_LIMIT_BYTES` (8 KiB). `PUT /v1/liveitems/{event_id}/artwork/{sha256}` has `artwork_max_bytes` from config. Tests: `a_body_over_8_kib_gives_413`, `a_body_over_the_limit_gives_413`. |
| An image is served only with the hash that its bytes have. | Pass | `upload_artwork` at src/lib.rs:966 compares the body hash with the path hash. Mismatch returns `400 sha256_mismatch`. Read returns the stored bytes at src/lib.rs:1014–1017. Test: `a_wrong_hash_gives_400_sha256_mismatch`. |
| The relay never fetches an image from a URL. | Pass | `DisplayInner.publish` at src/lib.rs:1629 checks `artwork.sha256` only, not `artwork.url`. The code never makes a network call. Test: `a_display_publish_with_an_image_that_is_not_held_gives_409_artwork_missing`. |
| No display data goes into the live value transports. | Pass | `publish_display` at src/lib.rs:881 writes only to `event.display`. Live value routes read only from `event.inner`. Test: `no_display_event_appears_on_the_live_value_transports`. |
| An ephemeral event gives `409 event_not_reserved` on each display route. | Pass | `get_reserved_entry` at src/lib.rs:845 checks class and returns `409 event_not_reserved` for ephemeral events. Tests: `an_ephemeral_event_gives_409_on_each_route`, `an_ephemeral_event_gives_409_event_not_reserved_on_both_routes`. |
| An upload with a wrong hash, with bytes that are not JPEG or PNG, and over the limit each give the correct error. | Pass | `sha256_mismatch` test: `a_wrong_hash_gives_400_sha256_mismatch`. Image type test: `bytes_that_are_not_jpeg_or_png_give_400_unsupported_image`. Body limit test: `the_default_limit_accepts_524288_bytes_and_refuses_one_more`. `image_mime` at src/lib.rs checks first bytes (JPEG magic 0xFF 0xD8 0xFF, PNG magic 0x89 0x50 0x4E 0x47 0x0D 0x0A 1A 0x0A). |
| A display publish with an unknown image gives `409 artwork_missing`. | Pass | `DisplayInner.publish` at src/lib.rs:1630 checks `image()` and returns `artwork_missing`. Test: `a_display_publish_with_an_image_that_is_not_held_gives_409_artwork_missing`. |
| A display state with an `http` or `https` URL is accepted with no upload. A `data:` or `file:` URL gives `400 invalid_display`. | Pass | `validate_display` in src/lib.rs checks artwork URLs. Tests: `a_wrong_shape_or_a_wrong_url_gives_400`. |
| A third image removes the first one. | Pass | `store_image` eviction logic at src/lib.rs:1597–1614. Test: `after_three_display_states_with_three_images_the_first_image_gives_404`. |
| A lease expiry sets the state to `{"track": null}`, sends it on SSE, and removes the images. | Pass | `clear_display_after_lease` at src/lib.rs:1326 sets state to `null_display_state()` and calls `remove_images()`. Tests: `a_lease_expiry_sets_the_state_to_null_and_sends_it`, `a_lease_expiry_with_a_snapshot_removes_the_images`. |
| A display publish does not renew the lease. | Pass | `publish_display` at src/lib.rs:881 does not call `renew_lease()`. It updates only `display`. The lease field `renewed_at` stays at its publish or keepalive time. Test: `a_display_publish_does_not_move_the_lease`. |
| No display event appears on `/events` or on Socket.IO. | Pass | `publish_display` sends to `event.display_sender` only. `/events` handler at src/lib.rs:2436 receives from `event.sender` only. Socket.IO handler at src/lib.rs:2478 emits from `listener_value` only. Test: `no_display_event_appears_on_the_live_value_transports`. |
| A client with no `Last-Event-ID` on `/display/events` first gets the present display state. | Pass | `subscribe_display` at src/lib.rs:1057–1061 builds a replay with the present state when `last_event_id` is absent. Test: `a_connect_with_no_last_event_id_first_gets_the_present_state`. |

## Status Codes

The task 001 and task 002 reviews accepted status codes. The test names match the README.md codes:

- `400 invalid_display`: A URL with wrong scheme, too long, or a MIME that mismatches the stored type. Test: `a_wrong_shape_or_a_wrong_url_gives_400`, `a_mime_that_is_not_the_stored_type_gives_400_invalid_display`.
- `400 sha256_mismatch`: Path hash does not match body hash. Test: `a_wrong_hash_gives_400_sha256_mismatch`.
- `400 unsupported_image`: Body does not start with JPEG or PNG bytes. Test: `bytes_that_are_not_jpeg_or_png_give_400_unsupported_image`.
- `401`, `403`, `404`: Bearer token errors and event not found. Test: `a_bad_credential_gives_401_or_403_and_an_unknown_event_gives_404`.
- `409 event_not_reserved`: Ephemeral event. Test: `an_ephemeral_event_gives_409_on_each_route`.
- `409 artwork_missing`: Display publish names an image the event does not hold. Test: `a_display_publish_with_an_image_that_is_not_held_gives_409_artwork_missing`.
- `413 payload_too_large`: Display body over 8 KiB or artwork body over limit. Tests: `a_body_over_8_kib_gives_413`, `a_body_over_the_limit_gives_413`.
- `429 publish_rate_limited`: Display publish or upload hits per-event rate limit. Tests: `the_display_publish_shares_the_publish_rate_limit`, `the_upload_shares_the_publish_rate_limit`.
- `503 max_sse_connections_reached`: Display stream count hits global SSE limit. Test: `the_display_stream_counts_against_the_sse_connection_limit`.

## Changes From ADR 0004

ADR 0004 changed the listener timeline for Socket.IO and `GET /remoteValue`. ADR 0003 display routes follow the instant timeline, not the delayed one. The routes do not take `listener_delay_secs` and send no pending updates. Display routes are instant:

- `POST /v1/liveitems/{event_id}/display` sends to SSE at once. No delay applies (src/lib.rs:881–910).
- `GET /v1/liveitems/{event_id}/display` reads the present state (src/lib.rs:924–928).
- `GET /v1/liveitems/{event_id}/display/events` is SSE instant (src/lib.rs:2362–2414).

The display-state and image tests all pass with ADR 0004 in place.

## Missing Tests

This point has no automatic test. It is not an ADR 0003 invariant.

- Socket.IO has no runtime test. Display never calls the Socket.IO emit. The display code has its own sender and never interacts with Socket.IO client state.

## Drift

- No drift found. `README.md` and `docs/interoperability.md` were updated in task 001 and task 002. The ADR text matches the implementation.

## Reconciliation With The Other Repositories

- `musicindex-live-publisher` ADR 0008 decides what the publisher sends on the display path. That ADR is `Accepted`. Publisher packet `display-path-task-004-publisher.md` sends the display states and the images. This review did not examine the publisher code.

## Merge Recommendation

Merge. Every invariant has a passing test, both task reviews found no defects, `Cargo.lock` has no change, and the full gate passes.

ADR 0003 becomes `Implemented`.

Follow-up work, with no gate on this ADR:

- Socket.IO runtime testing for the display path (if Socket.IO client tests are added later).
