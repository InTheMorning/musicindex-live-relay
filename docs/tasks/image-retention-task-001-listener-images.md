# Image Retention Task 001: The Listener Images

Status: Implemented - 2026-11-25.

Every criterion is mechanical.

## Goal

A reserved event keeps each image that its listener timeline names, in a
limit of `MAX_EVENT_IMAGES` images (ADR 0011). The limit of two images of
ADR 0003 stops.

## Base components

| Component | Status | Location | Reason |
|---|---|---|---|
| The image store of an event | extend | `src/lib.rs` | `DisplayInner::store_image` and `DisplayInner::publish` hold the retention rule of ADR 0003 |
| The listener timeline of an event | reuse | `src/lib.rs` | `LiveEventInner::listener_value` and `pending_listener_updates` hold each value that can name an image |
| The configuration read | extend | `src/lib.rs` | `AppConfig::from_env` and `parse_max_pending_listener_updates` are the pattern for a count with a lowest value |
| The HTTP test harness with the injected clock | reuse | `tests/api.rs` | The display tests and the listener timeline tests use it |

## Files To Inspect

- `docs/adr/0011-listener-timeline-image-retention.md`
- `docs/adr/0003-display-state-and-artwork.md`, §Storage And Lifetime
- `src/lib.rs`: `MAX_EVENT_IMAGES`, `AppConfig`, `parse_max_pending_listener_updates`,
  `RelayState::publish_display`, `RelayState::upload_artwork`, `LiveEventInner`,
  `PendingListenerUpdate`, `DisplayInner` and its functions, the unit tests
  of the configuration
- `tests/api.rs`: the image tests, for example
  `after_three_display_states_with_three_images_the_first_image_gives_404`,
  and the tests that set `Listener-Delay-Secs` with the injected clock
- `README.md`: "Image Retention" and the row `ARTWORK_MAX_BYTES` of the
  configuration table
- `docs/interoperability.md`: the list "Image retention keeps the memory in
  its limit"

## Files Likely To Change

- `src/lib.rs`
- `tests/api.rs`
- `README.md`
- `docs/interoperability.md`

## Do Not Touch

- Each route, each status code and each response body.
- The payload. The relay reads the `image` key and changes nothing.
- `LiveEventInner::apply_listener_update` and
  `RelayState::release_listener_updates`.
- `RelayState::clear_display_after_lease` and `DisplayInner::remove_images`.
  A lease end removes every image, as before.
- `Cargo.toml`, `docs/adr/**`, `AGENTS.md`, `docs/README.md`.

## Constraints

- `MAX_EVENT_IMAGES` becomes a configuration value: `AppConfig::max_event_images`,
  the constant `DEFAULT_MAX_EVENT_IMAGES` with the value 8 and the constant
  `MIN_EVENT_IMAGES` with the value 2. A value below 2 is a `ConfigError`.
  Follow `parse_max_pending_listener_updates`. Delete the constant
  `MAX_EVENT_IMAGES`.
- Add one free function that implements ADR 0011 §How A Payload Names An
  Image, for example
  `fn payload_artwork_sha256<'a>(value: &'a Value, event_id: &str) -> Option<&'a str>`.
  It reads only the top-level `image` string. It does not parse the start of
  the URL. It accepts no query and no text after the hash.
- Add one function that gives the listener images of an event, for example
  on `LiveEventInner`:
  `fn listener_artwork(&self, event_id: &str) -> HashSet<String>`. It reads
  `listener_value` and each pending update.
- Never hold the `inner` lock and the `display` lock at the same time.
  `publish_display` and `upload_artwork` take the `inner` read lock, copy the
  listener images, and release it. Then they take the `display` write lock.
  Correct the doc comment of `publish_display` that says "The live value lock
  is never taken".
- `DisplayInner::publish` gets the listener images and keeps each display
  image and each listener image (ADR 0011 §The Retention Rule).
- `DisplayInner::store_image` gets the listener images and the limit. At the
  limit it removes one image in the order of ADR 0011 §The Limit: group 1,
  then group 2, then the image of the earlier display state. In a group the
  earliest upload goes first. `images` is in upload order already. It never
  removes the image of the present display state.
- Do not store a hash in `PendingListenerUpdate`. Read the values at the time
  of the display publish and of the upload.
- Do not depend on a wall-clock sleep. Use the injected clock.
- Update each doc comment that states the limit of two images.

## Implementation Steps

1. Add the configuration value, its constants, its parse function and its
   unit tests.
2. Add the two functions of §Constraints, with unit tests for the URL forms.
3. Change `DisplayInner::publish` and `DisplayInner::store_image`, and their
   callers.
4. Change the existing tests that assert the limit of two images. Each one
   sets `max_event_images` to 2 in its configuration, or asserts the new
   limit. State in the report which test changed and why.
5. Add the tests of §Acceptance Criteria.
6. In `README.md`, write "Image Retention" again for ADR 0011, add the row
   `MAX_EVENT_IMAGES` to the configuration table, and correct the row
   `ARTWORK_MAX_BYTES`.
7. In `docs/interoperability.md`, write the list "Image retention keeps the
   memory in its limit" again for ADR 0011. State the present rule only, with
   no history.

## Acceptance Criteria

Mechanical. Each item is a test in `tests/api.rs`, unless the item names a
unit test:

- A payload with `Listener-Delay-Secs` names image A in `image`. Two
  subsequent tracks each upload an image and publish a display state. `GET`
  of image A gives `200` while the update is pending.
- After the clock passes the delay and the release runs, image A is in the
  present listener value. A third track uploads and publishes. `GET` of image
  A gives `200`.
- A different payload becomes the present listener value. After the next
  display publish, `GET` of image A gives `404 artwork_not_found`.
- Unit test: the function of §How A Payload Names An Image gives the hash for
  a correct URL with any start. It gives `None` for a different event
  identifier, for uppercase hexadecimal characters and for 63 characters. It
  also gives `None` for a query after the hash, for an `image` that is not a
  string, and for a value with no `image`.
- With `max_event_images` 3: the event holds the present display image, the
  earlier display image and one listener image. An upload of a fourth image
  removes the listener image, and the two display images give `200`.
- With `max_event_images` 4: the event holds the two display images, one
  listener image and one image that nothing names. An upload of a fifth image
  removes the image that nothing names, and the listener image gives `200`.
- With `max_event_images` 2: an upload removes the image of the earlier
  display state, and the image of the present display state gives `200`.
- Uploads with no display publish never make the event hold more than
  `max_event_images` images.
- Unit test: `MAX_EVENT_IMAGES` has the default 8 and reads the environment.
  The values 0 and 1 are a configuration error.
- A lease expiry removes a listener image: `GET` gives
  `404 artwork_not_found`.
- A payload whose `image` names an image that the event does not hold is
  accepted with `200`.

Also: the full gate passes.

## Test Commands

```bash
cargo fmt -- --check
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

## Escalation Triggers

- The change needs the `inner` lock and the `display` lock at the same time.
- An existing test that does not concern the image limit must change its
  assertion.
- The change needs a new dependency or a change to a route.

## Prompt for lower-context coding model

You are implementing one bounded task from a larger plan.

Implement only this task. Do not redesign the architecture.

Read:
- AGENTS.md
- docs/adr/0011-listener-timeline-image-retention.md
- docs/tasks/image-retention-task-001-listener-images.md
- the files of §Files To Inspect of the packet

Goal:
- A reserved event keeps each image that its listener timeline names, in a limit of MAX_EVENT_IMAGES images.

Constraints:
- Follow §Constraints of the packet exactly. Never hold the inner lock and the display lock at the same time.
- Write each comment and each document change in ASD-STE100 Simplified Technical English. Read /home/citizen/.agents/skills/asd-ste100/SKILL.md before you write prose, and run its checker on each changed document.
- Do not commit. Leave the change in the working tree.

Do not touch:
- the routes, the payload, apply_listener_update, release_listener_updates, clear_display_after_lease, Cargo.toml, docs/adr/**, AGENTS.md, docs/README.md

Acceptance criteria:
- Each mechanical item in §Acceptance Criteria of the packet is a passing test.

Test commands:
- cargo fmt -- --check
- cargo build
- cargo test
- cargo clippy --all-targets -- -D warnings

At the end, report:
1. files changed
2. tests run
3. behavior changed
4. deviations from task
5. unresolved concerns

## Review Result

Reviewed 2026-11-25. The full gate passes, with 72 unit tests and 155 tests
in `tests/api.rs`.

- A ChatGPT run made the change from this packet. The main session read the
  diff, corrected doc comments and document text, and added the test for a
  payload that names an image that the event does not hold.
- The existing tests of the limit of two images did not change their
  assertions. Their harness now sets `max_event_images` to 2.
- An independent review found that `payload_artwork_sha256` copied the check
  of `is_sha256_hex`. The function now calls `is_sha256_hex`.
- The same review reported a payload that arrives between the read of the
  listener images and the change of the store. The main session made no
  code change. The result is the same as a payload that arrives after the
  display publish, and ADR 0011 §The Retention Rule states that order.
