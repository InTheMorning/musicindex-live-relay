# Display Pairing Task 001: The Song Line And The Value Identity

Status: Implemented - 2026-10-06.

Every criterion is mechanical.

## Goal

A display track can carry `songLine` and `value {eventGuid, blockGuid}`. The
relay validates them and passes them through without a change.

## Files To Inspect

- `docs/adr/0005-display-song-line-and-value.md`
- `docs/adr/0003-display-state-and-artwork.md` (§The Display State)
- `src/lib.rs`: `validate_display`, `is_valid_artwork_url`
- `tests/api.rs` (the display tests)
- `README.md` (the display route), `docs/interoperability.md`

## Files Likely To Change

- `src/lib.rs`
- `tests/api.rs`
- `README.md`
- `docs/interoperability.md`

## Do Not Touch

- The live value routes, Socket.IO and the listener timeline
- The image store and `409 artwork_missing`
- The body limit of the display route
- `docs/adr/**`

## Constraints

- `validate_display` accepts a track with `artist`, `title` and `artwork`,
  and also with either or both of `songLine` and `value`. Any other key gives
  `400 invalid_display`.
- `songLine` is a string of 1 to 1,024 characters. Count characters, not
  bytes.
- `value` is an object with exactly the keys `eventGuid` and `blockGuid`.
  Each is a string of 1 to 128 characters.
- The stored and the sent state are the body as received. Do not assemble the
  track again, and do not change the sequence of its keys.

## Implementation Steps

1. Change the key count check of `validate_display` to the rules above.
2. Add the checks for the two keys.
3. Add the tests.
4. Update the display track shape in `README.md` and
   `docs/interoperability.md`.

## Acceptance Criteria

Each item is a test in `tests/api.rs`:

- A track with `songLine` and `value` is accepted. `GET /display` gives both
  keys without a change.
- A track with only `songLine`, and a track with only `value`, are accepted.
- An SSE subscriber of `/display/events` receives both keys.
- Each of these gives `400 invalid_display`:
  - an empty `songLine`, a `songLine` of 1,025 characters, and a `songLine`
    that is not a string,
  - a `value` with a third key, a `value` with an empty `blockGuid`, and a
    `blockGuid` of 129 characters,
  - an unknown track key.
- The existing display tests pass with no change.

Also: the full gate passes.

## Test Commands

```bash
cargo fmt -- --check
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

## Escalation Triggers

- The display state is rebuilt from typed fields somewhere, so an unknown key
  would be lost.
- An existing test asserts that a fourth track key is refused.

## Prompt for lower-context coding model

You are implementing one bounded task from a larger plan.

Implement only this task. Do not redesign the architecture.

Read:
- docs/adr/0005-display-song-line-and-value.md
- docs/adr/0003-display-state-and-artwork.md
- docs/tasks/display-pairing-task-001-song-line-and-value.md
- src/lib.rs
- tests/api.rs
- README.md, docs/interoperability.md

Goal:
- Accept the optional track keys songLine and value in a display state, validate them, and pass them through.

Constraints:
- Follow §Constraints of the packet exactly.

Do not touch:
- The live value routes, Socket.IO, the listener timeline, the image store, the body limit, docs/adr/**

Acceptance criteria:
- Each item in §Acceptance Criteria of the packet is a passing test.

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

Reviewed 2026-10-06. `Cargo.lock` did not change. The full gate passes.

No existing test changed. The review made four changes to the first
implementation:

- Two constants, `MAX_SONG_LINE_CHARS` and `MAX_VALUE_GUID_CHARS`, replace
  the literal limits.
- One helper, `is_text_of_length`, checks `songLine`, `eventGuid` and
  `blockGuid`.
- A test publishes a `songLine` of 1,024 two-byte characters. It shows that
  the limit counts characters, not bytes.
- `docs/interoperability.md` gives the track shape, as step 4 of this packet
  says.
